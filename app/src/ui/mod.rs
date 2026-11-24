pub mod dialogs;
pub mod info_pane;
pub mod preview_pane;
pub mod tasks_pane;
pub mod theme;
pub mod thumb_pane;
pub mod tree_pane;

use crate::catalog::{CatalogDb, CatalogStats, DuplicateGroup};
use crate::config::{
    AppSettings, AppTheme, LayoutPreset, SearchScope, SortField, ViewMode, MAX_MD5_FILE_KB,
};
use crate::decoder::{self, ImageFilter};
use crate::drag_drop::{self, DragOutcome};
use crate::folder_watch::FolderWatcher;
use crate::ghostscript;
use crate::keyword_writer;
use crate::sample_gen;
use crate::similarity::{self, DEFAULT_COLOR_SIMILARITY_WEIGHT};
use crate::worker::{
    self, BackgroundWorker, FolderScanJob, FolderScanner, PreviewRequest, PreviewWorker, ScanEvent,
    TaskControl, TaskState, ThumbDecodePool, ThumbDecodeRequest, ThumbEncodeConfig, WorkerEvent,
    WorkerRequest,
};
use egui::{ColorImage, RichText, TextureHandle, TextureOptions};
use image::DynamicImage;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq, Eq)]
struct VisibleCacheKey {
    items_revision: u64,
    items_structural_rev: u64,
    items_len: usize,
    search_query: String,
    format_filter: String,
    min_rating_filter: u8,
    only_with_keywords: bool,
    sort_field: SortField,
    sort_descending: bool,
    browse_kind: u8,
    similarity_sort_epoch: u64,
    similarity_batch_done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeTab {
    Folders,
    Keywords,
    Galleries,
    Filters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsTab {
    Ghostscript,
    Layout,
    Thumbnails,
    Tasks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseSource {
    Folder(PathBuf),
    Keyword(String),
    Gallery(String),
    Similar(PathBuf),
    Duplicates,
}

#[derive(Debug, Clone)]
pub struct FolderPropertiesInfo {
    pub path: PathBuf,
    pub direct_supported: usize,
    pub direct_subdirs: usize,
    pub cataloged_in_tree: usize,
    pub direct_bytes: u64,
}

#[derive(Clone)]
pub struct ManagedTask {
    pub id: u64,
    pub task_name: String,
    pub display_text: String,
    pub root_dir: PathBuf,
    pub started_at: String,
    pub completed_at: String,
    pub elapsed_ms: u64,
    pub active_ms: u64,
    pub thread_id: u64,
    pub state: TaskState,
    pub done: usize,
    pub total: usize,
    pub seen: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub current_file: String,
    pub messages: Vec<String>,
    pub errors: Vec<String>,
    pub control: Arc<TaskControl>,
}

#[derive(Clone)]
pub struct NativeIconCacheEntry {
    pub texture: Option<TextureHandle>,
    pub type_name: String,
}

#[derive(Clone)]
pub struct MediaItem {
    pub full_path: PathBuf,
    pub file_name: String,
    pub filetype: String,
    pub is_media: bool,
    pub width: u32,
    pub height: u32,
    pub color_depth: u16,
    pub page_count: u32,
    pub file_length: u64,
    pub file_time_iso: String,
    pub rating: u8,
    pub annotation: String,
    pub keywords: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub md5: String,
    pub similarity_score: Option<f32>,
    pub duplicate_group: Option<usize>,
    pub source_note: String,
    pub loaded: bool,
    pub thumb_blob: Option<(Arc<Vec<u8>>, String)>,
    pub texture: Option<TextureHandle>,
    pub last_drawn_frame: u64,
}

pub struct ThumbCraftApp {
    pub settings: AppSettings,
    pub base_dir: PathBuf,
    pub db_path: PathBuf,
    pub db: Arc<Mutex<CatalogDb>>,
    pub sample_dir: PathBuf,
    pub browse_source: BrowseSource,
    pub last_folder_context: PathBuf,
    pub history_back: Vec<BrowseSource>,
    pub history_fwd: Vec<BrowseSource>,
    pub path_bar_input: String,
    pub tree_tab: TreeTab,
    pub expanded_dirs: HashSet<PathBuf>,

    pub items: Vec<MediaItem>,
    pub path_to_index: HashMap<PathBuf, usize>,
    pub selected_index: Option<usize>,
    pub selected_indices: HashSet<usize>,
    pub ui_frame: u64,
    pub live_texture_count: usize,

    // UI-thread performance caches & async workers
    items_revision: u64,
    items_structural_rev: u64,
    tex_decode_gen: u64,
    cached_visible_key: Option<VisibleCacheKey>,
    pub cached_visible_indices: Vec<usize>,
    pub cached_roots: Vec<PathBuf>,
    pub cached_roots_at: Instant,
    pub cached_quick_access: Vec<(String, PathBuf)>,
    pub dir_children_cache: HashMap<PathBuf, (Instant, Vec<PathBuf>)>,
    checkerboard_tex: Option<TextureHandle>,
    preview_worker: PreviewWorker,
    preview_generation: u64,
    thumb_decode_pool: ThumbDecodePool,
    pending_tex_decodes: HashSet<usize>,
    applied_theme: Option<AppTheme>,
    cached_palette: theme::ThemePalette,
    pub native_icon_cache: HashMap<String, NativeIconCacheEntry>,

    // Filters & search
    pub search_query: String,
    pub format_filter: String,
    pub min_rating_filter: u8,
    pub only_with_keywords: bool,
    pub keyword_filter_text: String,

    // Catalog summaries
    pub all_keywords: Vec<(String, usize)>,
    pub all_galleries: Vec<(String, usize)>,
    pub catalog_stats: CatalogStats,
    pub new_gallery_name: String,
    pub new_keyword_input: String,
    pub keyword_sync_status: Option<(bool, String)>,
    pub annotation_draft: String,

    // Preview / Lightbox
    pub preview_page: usize,
    pub preview_zoom: f32,
    pub lightbox_zoom: f32,
    pub active_filter: ImageFilter,
    pub preview_raw_image: Option<DynamicImage>,
    pub preview_texture: Option<TextureHandle>,
    pub show_lightbox: bool,

    // Preferences modal
    pub show_preferences: bool,
    pub prefs_tab: PrefsTab,

    // Similarity & Hash Duplicate finder state
    pub similarity_threshold: f32,
    pub similarity_color_weight: f32,
    pub similar_only_same_type: bool,
    pub similar_dialog_target: Option<PathBuf>,
    pub similarity_sort_anchor: Option<PathBuf>,
    pub similarity_sort_epoch: u64,
    pub duplicate_groups: Vec<DuplicateGroup>,
    pub duplicate_scan_count: usize,

    // File & Folder operations modals
    pub rename_target: Option<PathBuf>,
    pub rename_input: String,
    pub rename_just_opened: bool,
    pub rename_ext_confirm: Option<(PathBuf, PathBuf)>,
    pub delete_targets: Vec<PathBuf>,
    pub folder_create_parent: Option<PathBuf>,
    pub folder_create_input: String,
    pub folder_rename_target: Option<PathBuf>,
    pub folder_rename_input: String,
    pub folder_delete_target: Option<PathBuf>,
    pub folder_props_modal: Option<FolderPropertiesInfo>,

    // Browse Worker & Managed Background Task Queue
    pub worker: BackgroundWorker,
    pub generation: u64,
    pub worker_done: usize,
    pub worker_total: usize,
    pub worker_current_file: String,
    pub worker_items_since_refresh: usize,

    pub scanner: FolderScanner,
    pub next_task_id: u64,
    pub managed_tasks: Vec<ManagedTask>,
    pub selected_task_id: Option<u64>,
    pub task_props_id: Option<u64>,

    pub task_log: Vec<String>,
    pub gs_version: Option<String>,
    pub gpu_info: Option<crate::gpu_accel::GpuAdapterInfo>,

    // Active folder auto-reload watcher & drag-and-drop state
    pub folder_watcher: FolderWatcher,
    pub drag_candidate: Option<(usize, egui::Pos2)>,
    pub internal_drag_files: Vec<PathBuf>,
    pub internal_drag_hover_dir: Option<PathBuf>,
    pub scanned_folders: HashSet<String>,

    // Screenshot automation
    pub screenshot_target: Option<PathBuf>,
    pub frame_count: u32,
    pub screenshot_requested: bool,
    pub egui_ctx: Option<egui::Context>,
}

impl ThumbCraftApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_folder: Option<PathBuf>,
        screenshot_target: Option<PathBuf>,
    ) -> Self {
        let mut settings = AppSettings::load();
        if settings.ghostscript.gs_path.is_empty() {
            settings.ghostscript.gs_path = ghostscript::detect_ghostscript().unwrap_or_default();
        }
        let gs_version = ghostscript::detect_version(&settings.ghostscript.gs_path);
        let gpu_info = crate::gpu_accel::gpu_adapter_info();

        let base_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        let db_path = settings
            .custom_db_path
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| base_dir.join("thumbcraft.tcdb"));

        let sample_dir = sample_gen::ensure_sample_library(&base_dir);

        let mut catalog_db = CatalogDb::open(&db_path)
            .or_else(|_| CatalogDb::open(&base_dir.join("thumbcraft.tcdb")))
            .expect("Failed to open thumbcraft.tcdb");
        let actual_db_path = catalog_db.path().to_path_buf();
        let migrated_count = catalog_db.migrated_legacy_count;
        let _ = catalog_db.create_gallery("Favorites");
        let _ = catalog_db.create_gallery("CDR Designs");
        let db = Arc::new(Mutex::new(catalog_db));

        let start_folder = initial_folder
            .filter(|p| p.is_dir())
            .or_else(|| {
                if screenshot_target.is_some() {
                    Some(sample_dir.clone())
                } else {
                    settings
                        .last_folder
                        .as_ref()
                        .map(PathBuf::from)
                        .filter(|p| p.is_dir())
                }
            })
            .unwrap_or_else(|| sample_dir.clone());

        let mut expanded_dirs = HashSet::new();
        if let Some(root) = start_folder.ancestors().last() {
            expanded_dirs.insert(root.to_path_buf());
        }

        let mut task_log = Vec::new();
        if let Some(ref v) = gs_version {
            task_log.push(format!(
                "Ghostscript {v} detected at {}",
                settings.ghostscript.gs_path
            ));
        } else {
            task_log.push(
                "Ghostscript not found in PATH or C:\\Program Files\\gs — configure in Preferences."
                    .to_string(),
            );
        }
        if migrated_count > 0 {
            task_log.push(format!(
                "Migrated {migrated_count} legacy thumbnail(s) into single compressed database."
            ));
        }
        task_log.push(format!(
            "Database ready: {} ({} Q{}, {}px)",
            actual_db_path.display(),
            settings.thumb_format.short_name(),
            settings.thumb_quality,
            settings.thumb_cache_dim
        ));

        let initial_palette = theme::ThemePalette::for_theme(settings.theme);
        let initial_roots = tree_pane::list_roots_uncached();
        let initial_quick_access = tree_pane::list_quick_access_uncached();

        let mut app = Self {
            settings,
            base_dir,
            db_path: actual_db_path,
            db,
            sample_dir,
            browse_source: BrowseSource::Folder(start_folder.clone()),
            last_folder_context: start_folder.clone(),
            history_back: Vec::new(),
            history_fwd: Vec::new(),
            path_bar_input: start_folder.to_string_lossy().to_string(),
            tree_tab: TreeTab::Folders,
            expanded_dirs,
            items: Vec::new(),
            path_to_index: HashMap::new(),
            selected_index: None,
            selected_indices: HashSet::new(),
            ui_frame: 0,
            live_texture_count: 0,
            items_revision: 1,
            items_structural_rev: 1,
            tex_decode_gen: 1,
            cached_visible_key: None,
            cached_visible_indices: Vec::new(),
            cached_roots: initial_roots,
            cached_roots_at: Instant::now(),
            cached_quick_access: initial_quick_access,
            dir_children_cache: HashMap::new(),
            checkerboard_tex: None,
            preview_worker: PreviewWorker::spawn(),
            preview_generation: 0,
            thumb_decode_pool: ThumbDecodePool::spawn(),
            pending_tex_decodes: HashSet::new(),
            applied_theme: None,
            cached_palette: initial_palette,
            native_icon_cache: HashMap::new(),
            search_query: String::new(),
            format_filter: "ALL".to_string(),
            min_rating_filter: 0,
            only_with_keywords: false,
            keyword_filter_text: String::new(),
            all_keywords: Vec::new(),
            all_galleries: Vec::new(),
            catalog_stats: CatalogStats::default(),
            new_gallery_name: String::new(),
            new_keyword_input: String::new(),
            keyword_sync_status: None,
            annotation_draft: String::new(),
            preview_page: 1,
            preview_zoom: 0.0,
            lightbox_zoom: 0.0,
            active_filter: ImageFilter::None,
            preview_raw_image: None,
            preview_texture: None,
            show_lightbox: false,
            show_preferences: false,
            prefs_tab: PrefsTab::Ghostscript,
            similarity_threshold: 0.65,
            similarity_color_weight: DEFAULT_COLOR_SIMILARITY_WEIGHT,
            similar_only_same_type: false,
            similar_dialog_target: None,
            similarity_sort_anchor: None,
            similarity_sort_epoch: 0,
            duplicate_groups: Vec::new(),
            duplicate_scan_count: 0,
            rename_target: None,
            rename_input: String::new(),
            rename_just_opened: false,
            rename_ext_confirm: None,
            delete_targets: Vec::new(),
            folder_create_parent: None,
            folder_create_input: String::new(),
            folder_rename_target: None,
            folder_rename_input: String::new(),
            folder_delete_target: None,
            folder_props_modal: None,
            worker: BackgroundWorker::spawn(),
            generation: 0,
            worker_done: 0,
            worker_total: 0,
            worker_current_file: String::new(),
            worker_items_since_refresh: 0,
            scanner: FolderScanner::spawn(),
            next_task_id: 1,
            managed_tasks: Vec::new(),
            selected_task_id: None,
            task_props_id: None,
            task_log,
            gs_version,
            gpu_info,
            folder_watcher: FolderWatcher::new(),
            drag_candidate: None,
            internal_drag_files: Vec::new(),
            internal_drag_hover_dir: None,
            scanned_folders: HashSet::new(),
            screenshot_target,
            frame_count: 0,
            screenshot_requested: false,
            egui_ctx: Some(cc.egui_ctx.clone()),
        };

        app.refresh_catalog_lists();
        app.reload_current_source(false);
        app
    }

    pub fn default_db_path(&self) -> PathBuf {
        self.base_dir.join("thumbcraft.tcdb")
    }

    pub fn switch_database_path(&mut self, new_path: PathBuf) {
        if new_path == self.db_path {
            return;
        }
        // Flush current DB before copying/switching
        if let Ok(mut db) = self.db.lock() {
            let _ = db.flush();
        }
        if let Some(parent) = new_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // If destination does not exist yet, migrate current database file there so nothing is lost
        if !new_path.is_file() && self.db_path.is_file() {
            if let Err(e) = std::fs::copy(&self.db_path, &new_path) {
                self.push_log(format!(
                    "Failed to copy database to {}: {e}",
                    new_path.display()
                ));
                return;
            }
        }
        match CatalogDb::open(&new_path) {
            Ok(new_db) => {
                self.db = Arc::new(Mutex::new(new_db));
                self.db_path = new_path.clone();
                let def = self.default_db_path();
                self.settings.custom_db_path = if new_path == def {
                    None
                } else {
                    Some(new_path.to_string_lossy().to_string())
                };
                self.settings.save();
                self.push_log(format!("Switched database to {}", new_path.display()));
                self.refresh_catalog_lists();
                self.reload_current_source(false);
            }
            Err(e) => {
                self.push_log(format!(
                    "Failed to open database at {}: {e}",
                    new_path.display()
                ));
            }
        }
    }

    pub fn open_db_in_explorer(&mut self) {
        self.reveal_in_explorer(&self.db_path.clone());
    }

    fn thumb_encode_cfg(&self) -> ThumbEncodeConfig {
        ThumbEncodeConfig {
            format: self.settings.thumb_format,
            quality: self.settings.thumb_quality.clamp(25, 100),
            max_dim: self.settings.thumb_cache_dim.clamp(64, 320),
            use_gpu: self.settings.use_gpu_acceleration,
            cpu_threads: self.settings.cpu_worker_threads,
        }
    }

    pub fn push_log(&mut self, msg: String) {
        self.task_log.push(msg);
        if self.task_log.len() > 250 {
            self.task_log.remove(0);
        }
    }

    pub fn bump_items_revision(&mut self) {
        self.items_revision = self.items_revision.wrapping_add(1);
    }

    pub fn bump_items_structural_revision(&mut self) {
        self.items_structural_rev = self.items_structural_rev.wrapping_add(1);
        self.tex_decode_gen = self.tex_decode_gen.wrapping_add(1);
        self.cached_visible_key = None;
        self.bump_items_revision();
    }

    pub fn invalidate_dir_tree_cache(&mut self, dir: Option<&Path>) {
        if let Some(d) = dir {
            self.dir_children_cache.remove(d);
            if let Some(p) = d.parent() {
                self.dir_children_cache.remove(p);
            }
        } else {
            self.dir_children_cache.clear();
            if let Some(past) = Instant::now().checked_sub(Duration::from_secs(60)) {
                self.cached_roots_at = past;
            }
        }
    }

    pub fn checkerboard_texture_id(&mut self, ctx: &egui::Context) -> egui::TextureId {
        if let Some(ref tex) = self.checkerboard_tex {
            return tex.id();
        }
        const CELL: usize = 10;
        const DIM: usize = CELL * 2;
        let c1 = [44u8, 44, 44, 255];
        let c2 = [56u8, 56, 56, 255];
        let mut pixels = Vec::with_capacity(DIM * DIM * 4);
        for y in 0..DIM {
            for x in 0..DIM {
                let col = if ((x / CELL) + (y / CELL)) % 2 == 0 {
                    c1
                } else {
                    c2
                };
                pixels.extend_from_slice(&col);
            }
        }
        let img = ColorImage::from_rgba_unmultiplied([DIM, DIM], &pixels);
        let mut opts = TextureOptions::NEAREST;
        opts.wrap_mode = egui::TextureWrapMode::Repeat;
        let handle = ctx.load_texture("thumbcraft_checkerboard_tile", img, opts);
        let id = handle.id();
        self.checkerboard_tex = Some(handle);
        id
    }

    pub fn request_item_texture_async(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(item) = self.items.get(idx) else {
            return;
        };
        if item.texture.is_some() || self.pending_tex_decodes.contains(&idx) {
            return;
        }
        let Some((ref blob, ref fmt)) = item.thumb_blob else {
            return;
        };
        self.pending_tex_decodes.insert(idx);
        self.thumb_decode_pool.submit(ThumbDecodeRequest {
            generation: self.tex_decode_gen,
            index: idx,
            blob: Arc::clone(blob),
            fmt: fmt.clone(),
            ctx: Some(ctx.clone()),
        });
    }

    fn poll_thumb_decodes(&mut self, ctx: &egui::Context) {
        let events = self.thumb_decode_pool.drain_events();
        let mut uploaded = 0usize;
        for evt in events {
            if evt.generation != self.tex_decode_gen {
                continue;
            }
            self.pending_tex_decodes.remove(&evt.index);
            if let Some((w, h, raw)) = evt.rgba {
                if let Some(item) = self.items.get_mut(evt.index) {
                    if item.texture.is_none() {
                        if let Some(tex) = load_rgba_texture_clamped(
                            ctx,
                            format!("thumb_{}_{}", self.tex_decode_gen, evt.index),
                            w,
                            h,
                            &raw,
                            TextureOptions::LINEAR,
                        ) {
                            item.texture = Some(tex);
                            item.last_drawn_frame = self.ui_frame;
                            self.live_texture_count += 1;
                            uploaded += 1;
                        }
                    }
                }
            }
        }
        if uploaded > 0 {
            ctx.request_repaint();
        }
    }

    /// Assigns the native OS file icon texture to a non-media item, caching by extension
    /// (or per-file path for `.exe`/`.ico`/`.lnk`). Returns `true` if a fresh OS shell lookup
    /// was performed on this call (allowing callers to throttle cold lookups per frame).
    pub fn ensure_native_icon(&mut self, ctx: &egui::Context, idx: usize) -> bool {
        let Some((path, is_media, is_loaded, has_blob, has_tex)) = self
            .items
            .get(idx)
            .map(|it| {
                (
                    it.full_path.clone(),
                    it.is_media,
                    it.loaded,
                    it.thumb_blob.is_some(),
                    it.texture.is_some(),
                )
            })
        else {
            return false;
        };
        if has_tex || (is_media && (!is_loaded || has_blob)) {
            return false;
        }
        let cache_key = crate::shell_icon::icon_cache_key(&path);
        if let Some(entry) = self.native_icon_cache.get(&cache_key) {
            if let Some(item) = self.items.get_mut(idx) {
                item.texture = entry.texture.clone();
                if !is_media && !entry.type_name.is_empty() {
                    item.source_note = entry.type_name.clone();
                }
            }
            return false;
        }

        let entry = if let Some(icon) = crate::shell_icon::extract_native_file_icon(&path) {
            let cimg = ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            let tex = ctx.load_texture(
                format!("native_icon_{cache_key}"),
                cimg,
                TextureOptions::LINEAR,
            );
            NativeIconCacheEntry {
                texture: Some(tex),
                type_name: icon.type_name,
            }
        } else {
            NativeIconCacheEntry {
                texture: None,
                type_name: "File".to_string(),
            }
        };

        if let Some(item) = self.items.get_mut(idx) {
            item.texture = entry.texture.clone();
            if !entry.type_name.is_empty() {
                item.source_note = entry.type_name.clone();
            }
        }
        // Keep per-file icon cache bounded when browsing huge binary folders
        if self.native_icon_cache.len() > 1024 && cache_key.starts_with("path:") {
            self.native_icon_cache.retain(|k, _| !k.starts_with("path:"));
        }
        self.native_icon_cache.insert(cache_key, entry);
        true
    }

    pub fn evict_distant_textures(&mut self) {
        const MAX_LIVE_TEXTURES: usize = 1800;
        if self.live_texture_count <= MAX_LIVE_TEXTURES {
            return;
        }
        let cutoff = self.ui_frame.saturating_sub(60);
        let mut candidates: Vec<(u64, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.is_media && it.texture.is_some() && it.last_drawn_frame < cutoff)
            .map(|(i, it)| (it.last_drawn_frame, i))
            .collect();
        candidates.sort_by_key(|&(frame, _)| frame);
        let to_evict = self
            .live_texture_count
            .saturating_sub(MAX_LIVE_TEXTURES - 200);
        for (_, idx) in candidates.into_iter().take(to_evict) {
            if self.items[idx].texture.take().is_some() {
                self.live_texture_count = self.live_texture_count.saturating_sub(1);
            }
        }
    }

    pub fn browse_title(&self) -> String {
        match &self.browse_source {
            BrowseSource::Folder(p) => p.to_string_lossy().to_string(),
            BrowseSource::Keyword(k) => format!("Keyword: #{k}"),
            BrowseSource::Gallery(g) => format!("Gallery: {g}"),
            BrowseSource::Similar(p) => format!(
                "Similar to: {} ({})",
                p.file_name().and_then(|n| n.to_str()).unwrap_or("image"),
                self.settings.search_scope.label()
            ),
            BrowseSource::Duplicates => {
                format!("Duplicate Files — {}", self.settings.search_scope.label())
            }
        }
    }

    fn enqueue_managed_task(
        &mut self,
        task_name: String,
        display_text: String,
        root_dir: PathBuf,
        explicit_files: Option<Vec<PathBuf>>,
        recursive: bool,
        force_redecode: bool,
    ) {
        self.settings.layout.show_tasks_pane = true;
        let id = self.next_task_id;
        self.next_task_id += 1;
        let control = Arc::new(TaskControl::default());

        self.managed_tasks.push(ManagedTask {
            id,
            task_name: task_name.clone(),
            display_text: display_text.clone(),
            root_dir: root_dir.clone(),
            started_at: String::new(),
            completed_at: String::new(),
            elapsed_ms: 0,
            active_ms: 0,
            thread_id: 0,
            state: TaskState::Queued,
            done: 0,
            total: explicit_files.as_ref().map(|v| v.len()).unwrap_or(0),
            seen: 0,
            indexed: 0,
            skipped: 0,
            current_file: String::new(),
            messages: Vec::new(),
            errors: Vec::new(),
            control: control.clone(),
        });
        self.selected_task_id = Some(id);

        self.scanner.enqueue(FolderScanJob {
            task_id: id,
            display_text,
            root_dir,
            explicit_files,
            recursive,
            force_redecode,
            gs_config: self.settings.ghostscript.clone(),
            thumb_cfg: self.thumb_encode_cfg(),
            db: self.db.clone(),
            control,
        });
    }

    pub fn start_folder_scan(&mut self, dir: PathBuf, recursive: bool, force_redecode: bool) {
        if !dir.is_dir() {
            return;
        }
        let (task_name, prefix) = match (recursive, force_redecode) {
            (true, true) => ("Remake tree", "Remake Tree"),
            (false, true) => ("Remake folder", "Remake Folder"),
            (true, false) => ("Make tree", "Scan Tree"),
            (false, false) => ("Make folder", "Scan Folder"),
        };
        let display_text = format!("{prefix}: {}", dir.display());
        self.enqueue_managed_task(
            task_name.to_string(),
            display_text,
            dir,
            None,
            recursive,
            force_redecode,
        );
    }

    /// Rebuilds thumbnails ONLY for the currently selected file(s) (F3 / Right-click).
    pub fn rebuild_selected_thumbnails(&mut self) {
        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        if indices.is_empty() {
            if let Some(i) = self.selected_index {
                indices.push(i);
            }
        }
        indices.sort_unstable();
        let files: Vec<PathBuf> = indices
            .into_iter()
            .filter_map(|i| {
                self.items
                    .get(i)
                    .filter(|it| it.is_media)
                    .map(|it| it.full_path.clone())
            })
            .collect();
        if files.is_empty() {
            return;
        }
        let root_dir = files[0]
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.last_folder_context.clone());
        let display_text = if files.len() == 1 {
            format!(
                "Rebuild Selected: {}",
                files[0].file_name().and_then(|n| n.to_str()).unwrap_or("")
            )
        } else {
            format!("Rebuild Selected ({} files)", files.len())
        };
        self.enqueue_managed_task(
            "Rebuild thumbnails".to_string(),
            display_text,
            root_dir,
            Some(files),
            false,
            true,
        );
    }

    pub fn pause_task(&mut self, task_id: u64) {
        if let Some(t) = self.managed_tasks.iter_mut().find(|t| t.id == task_id) {
            if matches!(t.state, TaskState::Running | TaskState::Queued) {
                t.control.paused.store(true, Ordering::SeqCst);
                t.state = TaskState::Paused;
            }
        }
    }

    pub fn resume_task(&mut self, task_id: u64) {
        if let Some(t) = self.managed_tasks.iter_mut().find(|t| t.id == task_id) {
            if t.state == TaskState::Paused {
                t.control.paused.store(false, Ordering::SeqCst);
                t.state = TaskState::Running;
            }
        }
    }

    pub fn quit_task(&mut self, task_id: u64) {
        if let Some(t) = self.managed_tasks.iter_mut().find(|t| t.id == task_id) {
            if t.state.is_active() {
                t.control.cancelled.store(true, Ordering::SeqCst);
                t.control.paused.store(false, Ordering::SeqCst);
                t.state = TaskState::Cancelled;
            }
        }
    }

    pub fn pause_all_tasks(&mut self) {
        for t in &mut self.managed_tasks {
            if matches!(t.state, TaskState::Running | TaskState::Queued) {
                t.control.paused.store(true, Ordering::SeqCst);
                t.state = TaskState::Paused;
            }
        }
    }

    pub fn resume_all_tasks(&mut self) {
        for t in &mut self.managed_tasks {
            if t.state == TaskState::Paused {
                t.control.paused.store(false, Ordering::SeqCst);
                t.state = TaskState::Running;
            }
        }
    }

    pub fn quit_all_tasks(&mut self) {
        for t in &mut self.managed_tasks {
            if t.state.is_active() {
                t.control.cancelled.store(true, Ordering::SeqCst);
                t.control.paused.store(false, Ordering::SeqCst);
                t.state = TaskState::Cancelled;
            }
        }
    }

    pub fn remove_folder_from_tree(&mut self, dir: &Path) {
        let removed = self
            .db
            .lock()
            .ok()
            .and_then(|mut db| db.remove_folder_tree(dir).ok())
            .unwrap_or(0);
        self.push_log(format!(
            "Removed {} cataloged thumbnail(s) under {} from database.",
            removed,
            dir.display()
        ));
        self.refresh_catalog_lists();
    }

    pub fn remove_folder_orphans(&mut self, scope: Option<&Path>) {
        let removed = self
            .db
            .lock()
            .ok()
            .and_then(|mut db| db.remove_orphans(scope).ok())
            .unwrap_or(0);
        match scope {
            Some(dir) => self.push_log(format!(
                "Removed {removed} orphan entry(ies) under {}.",
                dir.display()
            )),
            None => self.push_log(format!(
                "Removed {removed} orphan entry(ies) across entire database."
            )),
        }
        self.refresh_catalog_lists();
    }

    pub fn compact_database(&mut self) {
        let res = self.db.lock().ok().map(|mut db| db.compact_with_report());
        match res {
            Some(Ok(rep)) => {
                self.push_log(format!(
                    "Compacted {}: {} -> {} (removed {} dangling thumbnail(s), {} unused keyword(s), {} unused folder(s)).",
                    self.db_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("thumbcraft.tcdb"),
                    thumb_pane::format_bytes(rep.bytes_before),
                    thumb_pane::format_bytes(rep.bytes_after),
                    rep.removed_thumbs,
                    rep.removed_keywords,
                    rep.removed_paths,
                ));
                self.refresh_catalog_lists();
            }
            Some(Err(e)) => self.push_log(format!("Compact database failed: {e}")),
            None => {}
        }
    }

    pub fn toggle_favorite_folder(&mut self, dir: &Path) {
        let s = dir.to_string_lossy().to_string();
        if let Some(pos) = self
            .settings
            .favorite_folders
            .iter()
            .position(|f| f.eq_ignore_ascii_case(&s))
        {
            self.settings.favorite_folders.remove(pos);
            self.push_log(format!("Removed from favorites: {s}"));
        } else {
            self.settings.favorite_folders.push(s.clone());
            self.push_log(format!("Added to favorites: {s}"));
        }
        self.settings.save();
    }

    pub fn begin_create_folder(&mut self, parent: PathBuf) {
        self.folder_create_parent = Some(parent);
        self.folder_create_input = "New Folder".to_string();
    }

    pub fn commit_create_folder(&mut self) {
        let Some(parent) = self.folder_create_parent.take() else {
            return;
        };
        let name = self.folder_create_input.trim().to_string();
        if name.is_empty() {
            return;
        }
        let target = parent.join(&name);
        match std::fs::create_dir_all(&target) {
            Ok(()) => {
                self.invalidate_dir_tree_cache(Some(&parent));
                self.expanded_dirs.insert(parent);
                self.push_log(format!("Created folder: {}", target.display()));
            }
            Err(e) => self.push_log(format!("Create folder failed: {e}")),
        }
    }

    pub fn begin_rename_folder(&mut self, dir: PathBuf) {
        self.folder_rename_input = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        self.folder_rename_target = Some(dir);
    }

    pub fn commit_rename_folder(&mut self) {
        let Some(old_dir) = self.folder_rename_target.take() else {
            return;
        };
        let new_name = self.folder_rename_input.trim().to_string();
        if new_name.is_empty() {
            return;
        }
        let Some(parent) = old_dir.parent() else {
            return;
        };
        let new_dir = parent.join(&new_name);
        if new_dir == old_dir {
            return;
        }
        match std::fs::rename(&old_dir, &new_dir) {
            Ok(()) => {
                self.invalidate_dir_tree_cache(Some(&old_dir));
                self.invalidate_dir_tree_cache(Some(&new_dir));
                if let Ok(mut db) = self.db.lock() {
                    let _ = db.rename_folder_tree(&old_dir, &new_dir);
                }
                self.push_log(format!("Renamed folder to {}", new_dir.display()));
                if matches!(&self.browse_source, BrowseSource::Folder(cur) if cur == &old_dir) {
                    self.open_folder(new_dir);
                }
                self.refresh_catalog_lists();
            }
            Err(e) => self.push_log(format!("Rename folder failed: {e}")),
        }
    }

    pub fn begin_delete_folder(&mut self, dir: PathBuf) {
        self.folder_delete_target = Some(dir);
    }

    pub fn commit_delete_folder(&mut self) {
        let Some(dir) = self.folder_delete_target.take() else {
            return;
        };
        let parent = dir.parent().map(|p| p.to_path_buf());
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {
                self.invalidate_dir_tree_cache(Some(&dir));
                if let Ok(mut db) = self.db.lock() {
                    let _ = db.remove_folder_tree(&dir);
                }
                self.push_log(format!("Deleted folder: {}", dir.display()));
                self.refresh_catalog_lists();
                if matches!(&self.browse_source, BrowseSource::Folder(cur) if cur == &dir) {
                    if let Some(p) = parent {
                        self.open_folder(p);
                    }
                }
            }
            Err(e) => self.push_log(format!("Delete folder failed: {e}")),
        }
    }

    pub fn copy_folder_to_dest(&mut self, src: &Path) {
        let Some(dest_parent) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let Some(name) = src.file_name() else {
            return;
        };
        let dst = dest_parent.join(name);
        match copy_dir_recursive(src, &dst) {
            Ok(n) => {
                self.invalidate_dir_tree_cache(Some(&dest_parent));
                self.push_log(format!("Copied folder ({n} file(s)) to {}", dst.display()));
            }
            Err(e) => self.push_log(format!("Copy folder failed: {e}")),
        }
    }

    pub fn move_folder_to_dest(&mut self, src: &Path) {
        let Some(dest_parent) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let Some(name) = src.file_name() else {
            return;
        };
        let dst = dest_parent.join(name);
        let res = std::fs::rename(src, &dst).or_else(|_| {
            copy_dir_recursive(src, &dst)?;
            std::fs::remove_dir_all(src)
        });
        match res {
            Ok(()) => {
                self.invalidate_dir_tree_cache(Some(src));
                self.invalidate_dir_tree_cache(Some(&dest_parent));
                if let Ok(mut db) = self.db.lock() {
                    let _ = db.rename_folder_tree(src, &dst);
                }
                self.push_log(format!("Moved folder to {}", dst.display()));
                if matches!(&self.browse_source, BrowseSource::Folder(cur) if cur == src) {
                    self.open_folder(dst);
                }
                self.refresh_catalog_lists();
            }
            Err(e) => self.push_log(format!("Move folder failed: {e}")),
        }
    }

    pub fn open_folder_in_explorer(&mut self, dir: &Path) {
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("explorer").arg(dir).spawn();
        }
        #[cfg(not(windows))]
        {
            let _ = open::that(dir);
        }
        self.push_log(format!("Opened in Explorer: {}", dir.display()));
    }

    pub fn open_folder_properties(&mut self, dir: PathBuf) {
        let mut direct_supported = 0usize;
        let mut direct_subdirs = 0usize;
        let mut direct_bytes = 0u64;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    direct_subdirs += 1;
                } else if p.is_file() {
                    if let Ok(md) = entry.metadata() {
                        direct_bytes += md.len();
                    }
                    if decoder::is_supported_file(&p) {
                        direct_supported += 1;
                    }
                }
            }
        }
        let cataloged_in_tree = self
            .db
            .lock()
            .ok()
            .map(|db| db.folder_catalog_count(&dir))
            .unwrap_or(0);
        self.folder_props_modal = Some(FolderPropertiesInfo {
            path: dir,
            direct_supported,
            direct_subdirs,
            cataloged_in_tree,
            direct_bytes,
        });
    }

    pub fn open_similar_dialog(&mut self, target_path: PathBuf) {
        self.similar_dialog_target = Some(target_path);
    }

    pub fn open_similar_view(&mut self, target_path: PathBuf) {
        let next = BrowseSource::Similar(target_path);
        if next != self.browse_source {
            self.history_back.push(self.browse_source.clone());
            self.history_fwd.clear();
        }
        self.browse_source = next;
        self.reload_current_source(false);
    }

    pub fn open_duplicates_view(&mut self) {
        let next = BrowseSource::Duplicates;
        if next != self.browse_source {
            self.history_back.push(self.browse_source.clone());
            self.history_fwd.clear();
        }
        self.browse_source = next;
        self.reload_current_source(false);
    }

    pub fn open_selected_externally(&mut self) {
        if let Some(item) = self.selected_item().cloned() {
            self.open_path_externally(&item.full_path);
        }
    }

    pub fn open_path_externally(&mut self, path: &Path) {
        match open::that(path) {
            Ok(()) => {
                self.push_log(format!(
                    "Opened externally: {}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
                ));
            }
            Err(e) => {
                self.push_log(format!("External open failed: {e}"));
            }
        }
    }

    pub fn reveal_in_explorer(&mut self, path: &Path) {
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,{}", path.to_string_lossy()))
                .spawn();
        }
        #[cfg(not(windows))]
        {
            if let Some(p) = path.parent() {
                let _ = open::that(p);
            }
        }
        self.push_log(format!(
            "Revealed in Explorer: {}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("")
        ));
    }

    pub fn begin_rename_selected(&mut self) {
        if let Some(item) = self.selected_item().cloned() {
            self.rename_input = item.file_name;
            self.rename_target = Some(item.full_path);
            self.rename_just_opened = true;
            self.rename_ext_confirm = None;
        }
    }

    pub fn commit_rename(&mut self) {
        let Some(old_path) = self.rename_target.take() else {
            return;
        };
        self.rename_just_opened = false;
        let new_name = self.rename_input.trim().to_string();
        if new_name.is_empty() {
            return;
        }
        let Some(parent) = old_path.parent() else {
            return;
        };
        let new_path = parent.join(&new_name);
        if new_path == old_path {
            return;
        }
        if needs_extension_change_warning(&old_path, &new_name) {
            self.rename_ext_confirm = Some((old_path, new_path));
            return;
        }
        self.execute_file_rename(&old_path, &new_path);
    }

    pub fn confirm_rename_with_extension_change(&mut self) {
        let Some((old_path, new_path)) = self.rename_ext_confirm.take() else {
            return;
        };
        self.execute_file_rename(&old_path, &new_path);
    }

    fn execute_file_rename(&mut self, old_path: &Path, new_path: &Path) {
        let new_name = new_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        match std::fs::rename(old_path, new_path) {
            Ok(()) => {
                if let Ok(mut db) = self.db.lock() {
                    let _ = db.rename_path(old_path, new_path);
                }
                self.push_log(format!("Renamed to {new_name}"));
                self.reload_current_source(false);
            }
            Err(e) => {
                self.push_log(format!("Rename failed: {e}"));
            }
        }
    }

    pub fn selected_file_paths(&self) -> Vec<PathBuf> {
        let mut indices: Vec<usize> = self.selected_indices.iter().copied().collect();
        if indices.is_empty() {
            if let Some(idx) = self.selected_index {
                indices.push(idx);
            }
        }
        indices.sort_unstable();
        indices.dedup();

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for idx in indices {
            if let Some(item) = self.items.get(idx) {
                if seen.insert(item.full_path.clone()) {
                    out.push(item.full_path.clone());
                }
            }
        }
        out
    }

    pub fn begin_internal_drag(&mut self, item_idx: usize) {
        if !self.is_item_selected(item_idx) {
            self.select_item(item_idx);
        }
        let paths = self.selected_file_paths();
        if !paths.is_empty() {
            self.internal_drag_files = paths;
            self.internal_drag_hover_dir = None;
        }
    }

    #[allow(dead_code)]
    pub fn is_folder_scanned(&self, dir: &Path) -> bool {
        let key = crate::catalog::normalize_dir_key(dir);
        !key.is_empty() && self.scanned_folders.contains(&key)
    }

    pub fn start_external_drag_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        match drag_drop::start_file_drag(&paths) {
            Ok(DragOutcome::Dropped) => {
                if paths.len() == 1 {
                    let name = paths[0]
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file");
                    self.push_log(format!("Dragged {name} to external application"));
                } else {
                    self.push_log(format!(
                        "Dragged {} files to external application",
                        paths.len()
                    ));
                }
            }
            Ok(DragOutcome::Cancelled) => {}
            Err(e) => {
                self.push_log(format!("External drag failed: {e}"));
            }
        }
    }

    pub fn move_files_to_folder(&mut self, files: &[PathBuf], dest_dir: &Path) {
        if !dest_dir.is_dir() || files.is_empty() {
            return;
        }
        let dest_key = crate::catalog::normalize_dir_key(dest_dir);
        let mut moved_pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut skipped_same = 0usize;
        let mut skipped_exists = Vec::new();
        let mut errors = Vec::new();

        for src in files {
            if !src.is_file() {
                continue;
            }
            if let Some(parent) = src.parent() {
                if crate::catalog::normalize_dir_key(parent) == dest_key {
                    skipped_same += 1;
                    continue;
                }
            }
            let Some(fname) = src.file_name() else {
                continue;
            };
            let dst = dest_dir.join(fname);
            if dst.exists() {
                skipped_exists.push(fname.to_string_lossy().to_string());
                continue;
            }
            let res = std::fs::rename(src, &dst).or_else(|_| {
                std::fs::copy(src, &dst)?;
                std::fs::remove_file(src)
            });
            match res {
                Ok(()) => moved_pairs.push((src.clone(), dst)),
                Err(e) => errors.push(format!("{}: {e}", fname.to_string_lossy())),
            }
        }

        if !moved_pairs.is_empty() {
            if let Ok(mut db) = self.db.lock() {
                let _ = db.rename_paths(&moved_pairs);
            }
            if moved_pairs.len() == 1 {
                let name = moved_pairs[0]
                    .0
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file");
                self.push_log(format!("Moved {name} to {}", dest_dir.display()));
            } else {
                self.push_log(format!(
                    "Moved {} files to {}",
                    moved_pairs.len(),
                    dest_dir.display()
                ));
            }
            self.selected_indices.clear();
            self.selected_index = None;
            self.refresh_catalog_lists();
            self.reload_current_source(false);
        } else if skipped_same > 0 && skipped_exists.is_empty() && errors.is_empty() {
            self.push_log("File(s) are already in that folder.".to_string());
        }

        if !skipped_exists.is_empty() {
            self.push_log(format!(
                "Skipped {} file(s) already in {}: {}",
                skipped_exists.len(),
                dest_dir.display(),
                skipped_exists.join(", ")
            ));
        }
        for err in errors {
            self.push_log(format!("Move failed ({err})"));
        }
    }

    pub fn begin_delete_selected(&mut self) {
        self.delete_targets = self.selected_file_paths();
    }

    pub fn commit_delete(&mut self) {
        let targets = std::mem::take(&mut self.delete_targets);
        if targets.is_empty() {
            return;
        }
        let mut deleted_paths = Vec::new();
        let mut failed_msgs = Vec::new();

        for path in &targets {
            match std::fs::remove_file(path) {
                Ok(()) => {
                    deleted_paths.push(path.clone());
                }
                Err(e) => {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default();
                    failed_msgs.push(format!("{name}: {e}"));
                }
            }
        }

        if !deleted_paths.is_empty() {
            // Do NOT touch or persist the database on delete to keep deletion instant.
            // Dangling thumbnails / unused keywords are cleaned up later by Compact Database.
            let deleted_set: HashSet<PathBuf> = deleted_paths.iter().cloned().collect();

            // Choose the nearest surviving item in visible order so selection stays natural
            let next_sel_path: Option<PathBuf> = {
                let _ = self.ensure_visible_indices_cached();
                let vis = &self.cached_visible_indices;
                let anchor_pos = self
                    .selected_index
                    .and_then(|sel| vis.iter().position(|&v| v == sel));
                if let Some(pos) = anchor_pos {
                    vis[pos + 1..]
                        .iter()
                        .chain(vis[..pos].iter().rev())
                        .find_map(|&idx| {
                            let p = &self.items[idx].full_path;
                            if !deleted_set.contains(p) {
                                Some(p.clone())
                            } else {
                                None
                            }
                        })
                } else {
                    None
                }
            };

            // Remove deleted items in-place without clearing remaining loaded thumbnails or re-scanning disk
            self.items.retain(|it| !deleted_set.contains(&it.full_path));
            self.path_to_index.clear();
            self.live_texture_count = 0;
            for (i, it) in self.items.iter().enumerate() {
                self.path_to_index.insert(it.full_path.clone(), i);
                if it.is_media && it.texture.is_some() {
                    self.live_texture_count += 1;
                }
            }
            self.pending_tex_decodes.clear();
            self.bump_items_structural_revision();

            self.selected_indices.clear();
            if let Some(next_idx) = next_sel_path
                .and_then(|p| self.path_to_index.get(&p).copied())
                .or_else(|| if self.items.is_empty() { None } else { Some(0) })
            {
                self.select_item(next_idx);
            } else {
                self.selected_index = None;
                self.preview_texture = None;
                self.preview_raw_image = None;
                self.annotation_draft.clear();
            }

            if deleted_paths.len() == 1 {
                self.push_log(format!(
                    "Deleted {}",
                    deleted_paths[0]
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                ));
            } else {
                self.push_log(format!("Deleted {} files", deleted_paths.len()));
            }
        }

        for msg in failed_msgs {
            self.push_log(format!("Delete failed ({msg})"));
        }
    }

    pub fn move_selected_files_to_folder(&mut self) {
        let targets = self.selected_file_paths();
        if targets.is_empty() {
            return;
        }
        let Some(dest_dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };

        let mut moved_pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut failed_msgs: Vec<String> = Vec::new();
        let mut skipped_same = 0usize;

        for src in &targets {
            let Some(fname) = src.file_name() else {
                continue;
            };
            let dst = dest_dir.join(fname);
            if src == &dst {
                skipped_same += 1;
                continue;
            }
            let res = std::fs::rename(src, &dst).or_else(|_| {
                std::fs::copy(src, &dst)?;
                std::fs::remove_file(src)
            });
            match res {
                Ok(()) => moved_pairs.push((src.clone(), dst)),
                Err(e) => failed_msgs.push(format!("{}: {e}", fname.to_string_lossy())),
            }
        }

        if !moved_pairs.is_empty() {
            if let Ok(mut db) = self.db.lock() {
                for (src, dst) in &moved_pairs {
                    let _ = db.rename_path(src, dst);
                }
            }

            let moved_set: HashSet<PathBuf> = moved_pairs.iter().map(|(s, _)| s.clone()).collect();
            let next_sel_path: Option<PathBuf> = {
                let _ = self.ensure_visible_indices_cached();
                let vis = &self.cached_visible_indices;
                let anchor_pos = self
                    .selected_index
                    .and_then(|sel| vis.iter().position(|&v| v == sel));
                if let Some(pos) = anchor_pos {
                    vis[pos + 1..]
                        .iter()
                        .chain(vis[..pos].iter().rev())
                        .find_map(|&idx| {
                            let it = self.items.get(idx)?;
                            if !moved_set.contains(&it.full_path) {
                                Some(it.full_path.clone())
                            } else {
                                None
                            }
                        })
                } else {
                    None
                }
            };

            self.items.retain(|it| !moved_set.contains(&it.full_path));
            self.path_to_index.clear();
            self.live_texture_count = 0;
            for (i, it) in self.items.iter().enumerate() {
                self.path_to_index.insert(it.full_path.clone(), i);
                if it.is_media && it.texture.is_some() {
                    self.live_texture_count += 1;
                }
            }
            self.pending_tex_decodes.clear();
            self.bump_items_structural_revision();

            self.selected_indices.clear();
            self.selected_index = next_sel_path
                .and_then(|p| self.path_to_index.get(&p).copied())
                .or_else(|| if self.items.is_empty() { None } else { Some(0) });
            if let Some(idx) = self.selected_index {
                self.selected_indices.insert(idx);
                self.annotation_draft = self.items[idx].annotation.clone();
                self.preview_page = 1;
                self.active_filter = ImageFilter::None;
                self.refresh_preview_image(self.show_lightbox);
            } else {
                self.preview_texture = None;
                self.preview_raw_image = None;
                self.annotation_draft.clear();
            }

            if moved_pairs.len() == 1 {
                let name = moved_pairs[0]
                    .0
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                self.push_log(format!("Moved {name} to {}", dest_dir.display()));
            } else {
                self.push_log(format!(
                    "Moved {} file(s) to {}",
                    moved_pairs.len(),
                    dest_dir.display()
                ));
            }
        } else if skipped_same > 0 && failed_msgs.is_empty() {
            self.push_log("Selected file(s) are already in the chosen folder.".to_string());
        }

        if !failed_msgs.is_empty() {
            self.push_log(format!("Move failed: {}", failed_msgs.join("; ")));
        }
    }

    pub fn copy_selected_files_to_folder(&mut self) {
        let targets = self.selected_file_paths();
        if targets.is_empty() {
            return;
        }
        let Some(dest_dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };

        let mut copied_count = 0usize;
        let mut last_name = String::new();
        let mut failed_msgs: Vec<String> = Vec::new();
        let mut skipped_same = 0usize;

        for src in &targets {
            let Some(fname) = src.file_name() else {
                continue;
            };
            let dst = dest_dir.join(fname);
            if src == &dst {
                skipped_same += 1;
                continue;
            }
            match std::fs::copy(src, &dst) {
                Ok(_) => {
                    copied_count += 1;
                    last_name = fname.to_string_lossy().to_string();
                }
                Err(e) => failed_msgs.push(format!("{}: {e}", fname.to_string_lossy())),
            }
        }

        if copied_count == 1 {
            self.push_log(format!("Copied {last_name} to {}", dest_dir.display()));
        } else if copied_count > 1 {
            self.push_log(format!(
                "Copied {copied_count} file(s) to {}",
                dest_dir.display()
            ));
        } else if skipped_same > 0 && failed_msgs.is_empty() {
            self.push_log("Selected file(s) are already in the chosen folder.".to_string());
        }

        if !failed_msgs.is_empty() {
            self.push_log(format!("Copy failed: {}", failed_msgs.join("; ")));
        }
    }

    pub fn open_folder(&mut self, folder: PathBuf) {
        if !folder.is_dir() {
            return;
        }
        let next = BrowseSource::Folder(folder.clone());
        if next != self.browse_source {
            self.history_back.push(self.browse_source.clone());
            self.history_fwd.clear();
        }
        self.browse_source = next;
        self.last_folder_context = folder.clone();
        self.path_bar_input = folder.to_string_lossy().to_string();
        self.settings.last_folder = Some(self.path_bar_input.clone());
        self.settings.save();
        self.reload_current_source(false);
    }

    pub fn open_keyword_view(&mut self, keyword: String) {
        let next = BrowseSource::Keyword(keyword);
        if next != self.browse_source {
            self.history_back.push(self.browse_source.clone());
            self.history_fwd.clear();
        }
        self.browse_source = next;
        self.reload_current_source(false);
    }

    pub fn open_gallery_view(&mut self, gallery: String) {
        let next = BrowseSource::Gallery(gallery);
        if next != self.browse_source {
            self.history_back.push(self.browse_source.clone());
            self.history_fwd.clear();
        }
        self.browse_source = next;
        self.reload_current_source(false);
    }

    pub fn go_back(&mut self) {
        if let Some(prev) = self.history_back.pop() {
            self.history_fwd.push(self.browse_source.clone());
            self.browse_source = prev;
            if let BrowseSource::Folder(ref p) = self.browse_source {
                self.last_folder_context = p.clone();
                self.path_bar_input = p.to_string_lossy().to_string();
            }
            self.reload_current_source(false);
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(next) = self.history_fwd.pop() {
            self.history_back.push(self.browse_source.clone());
            self.browse_source = next;
            if let BrowseSource::Folder(ref p) = self.browse_source {
                self.last_folder_context = p.clone();
                self.path_bar_input = p.to_string_lossy().to_string();
            }
            self.reload_current_source(false);
        }
    }

    pub fn go_up(&mut self) {
        if let BrowseSource::Folder(ref p) = self.browse_source {
            if let Some(parent) = p.parent() {
                self.open_folder(parent.to_path_buf());
            }
        }
    }

    fn current_search_scope_param(&self) -> Option<(PathBuf, bool)> {
        match self.settings.search_scope {
            SearchScope::EntireDatabase => None,
            SearchScope::CurrentTree => Some((self.last_folder_context.clone(), true)),
            SearchScope::CurrentFolder => Some((self.last_folder_context.clone(), false)),
        }
    }

    pub fn reload_current_source(&mut self, force_redecode: bool) {
        let mut sim_map: HashMap<PathBuf, f32> = HashMap::new();
        let mut dup_map: HashMap<PathBuf, usize> = HashMap::new();
        let scope_param = self.current_search_scope_param();

        let files: Vec<PathBuf> = match &self.browse_source {
            BrowseSource::Folder(dir) => {
                collect_folder_files(dir, self.settings.include_subfolders)
            }
            BrowseSource::Keyword(kw) => self
                .db
                .lock()
                .ok()
                .and_then(|db| db.find_paths_by_keyword(kw).ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.is_file())
                .collect(),
            BrowseSource::Gallery(gal) => self
                .db
                .lock()
                .ok()
                .and_then(|db| db.get_gallery_paths(gal).ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.is_file())
                .collect(),
            BrowseSource::Similar(target_path) => {
                let scope_ref = scope_param.as_ref().map(|(d, rec)| (d.as_path(), *rec));
                let only_ext = if self.similar_only_same_type {
                    target_path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_ascii_uppercase())
                } else {
                    None
                };
                let matches = self
                    .db
                    .lock()
                    .ok()
                    .and_then(|mut db| {
                        db.find_similar_to(
                            target_path,
                            self.similarity_threshold,
                            self.similarity_color_weight,
                            scope_ref,
                            only_ext.as_deref(),
                        )
                        .ok()
                    })
                    .unwrap_or_default();
                self.push_log(format!(
                    "Visual similarity ({}) found {} match(es) >= {:.0}% (color weight {:.0}%).",
                    self.settings.search_scope.label(),
                    matches.len(),
                    self.similarity_threshold * 100.0,
                    self.similarity_color_weight * 100.0
                ));
                let mut paths = Vec::with_capacity(matches.len());
                for m in matches {
                    sim_map.insert(m.path.clone(), m.similarity);
                    paths.push(m.path);
                }
                paths
            }
            BrowseSource::Duplicates => {
                let scope_ref = scope_param.as_ref().map(|(d, rec)| (d.as_path(), *rec));
                let (groups, scanned) = self
                    .db
                    .lock()
                    .ok()
                    .and_then(|mut db| db.find_hash_duplicates(scope_ref).ok())
                    .unwrap_or_else(|| (Vec::new(), 0));
                let mut paths = Vec::new();
                for g in &groups {
                    for p in &g.paths {
                        dup_map.insert(p.clone(), g.group_index);
                        paths.push(p.clone());
                    }
                }
                self.push_log(format!(
                    "MD5 duplicate scan ({}) checked {} file(s): found {} group(s) ({} file(s)).",
                    self.settings.search_scope.label(),
                    scanned,
                    groups.len(),
                    paths.len()
                ));
                self.duplicate_groups = groups;
                self.duplicate_scan_count = scanned;
                paths
            }
        };

        self.path_to_index.clear();
        self.live_texture_count = 0;
        let mut media_files: Vec<PathBuf> = Vec::new();
        self.items = files
            .iter()
            .enumerate()
            .map(|(i, p)| {
                self.path_to_index.insert(p.clone(), i);
                let is_media = decoder::is_supported_file(p);
                if is_media {
                    media_files.push(p.clone());
                }
                let fname = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_uppercase())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "FILE".to_string());
                let (file_length, file_time_iso) = crate::worker::file_stat(p);
                MediaItem {
                    full_path: p.clone(),
                    file_name: fname,
                    filetype: ext,
                    is_media,
                    width: 0,
                    height: 0,
                    color_depth: if is_media { 24 } else { 0 },
                    page_count: if is_media { 1 } else { 0 },
                    file_length,
                    file_time_iso,
                    rating: 0,
                    annotation: String::new(),
                    keywords: Vec::new(),
                    metadata: BTreeMap::new(),
                    md5: String::new(),
                    similarity_score: sim_map.get(p).copied(),
                    duplicate_group: dup_map.get(p).copied(),
                    source_note: if is_media {
                        "Queued".to_string()
                    } else {
                        "Native File Icon".to_string()
                    },
                    loaded: !is_media,
                    thumb_blob: None,
                    texture: None,
                    last_drawn_frame: 0,
                }
            })
            .collect();

        self.selected_indices.clear();
        if self.items.is_empty() {
            self.selected_index = None;
        } else {
            self.selected_index = Some(0);
            self.selected_indices.insert(0);
        }
        self.preview_texture = None;
        self.preview_raw_image = None;
        self.preview_page = 1;
        self.active_filter = ImageFilter::None;
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.pending_tex_decodes.clear();
        self.bump_items_structural_revision();

        self.generation += 1;
        self.worker_done = 0;
        self.worker_total = media_files.len();
        self.worker_current_file.clear();
        self.worker_items_since_refresh = 0;

        if !media_files.is_empty() {
            let send_blob_only = media_files.len() > 400;
            let cache_only = !self.settings.auto_scan && !force_redecode;
            self.worker.submit(WorkerRequest {
                files: media_files,
                gs_config: self.settings.ghostscript.clone(),
                thumb_cfg: self.thumb_encode_cfg(),
                db: self.db.clone(),
                force_redecode,
                generation: self.generation,
                send_blob_only,
                cache_only,
            });
        }
    }

    pub fn is_item_selected(&self, index: usize) -> bool {
        self.selected_indices.contains(&index) || self.selected_index == Some(index)
    }

    pub fn select_item(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        self.selected_indices.clear();
        self.selected_indices.insert(index);
        self.selected_index = Some(index);
        self.annotation_draft = self.items[index].annotation.clone();
        self.preview_page = 1;
        self.active_filter = ImageFilter::None;
        self.refresh_preview_image(self.show_lightbox);
    }

    pub fn toggle_select_item(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        if self.selected_indices.contains(&index) {
            self.selected_indices.remove(&index);
            if self.selected_index == Some(index) {
                self.selected_index = self.selected_indices.iter().next().copied();
            }
        } else {
            self.selected_indices.insert(index);
            self.selected_index = Some(index);
            self.annotation_draft = self.items[index].annotation.clone();
            self.preview_page = 1;
            self.active_filter = ImageFilter::None;
            self.refresh_preview_image(self.show_lightbox);
        }
    }

    pub fn select_range_to(&mut self, target_idx: usize, visible: &[usize]) {
        if target_idx >= self.items.len() {
            return;
        }
        let anchor = self.selected_index.unwrap_or(target_idx);
        let pos_a = visible.iter().position(|&v| v == anchor);
        let pos_b = visible.iter().position(|&v| v == target_idx);
        if let (Some(a), Some(b)) = (pos_a, pos_b) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            for &idx in &visible[lo..=hi] {
                self.selected_indices.insert(idx);
            }
            self.selected_index = Some(target_idx);
            self.annotation_draft = self.items[target_idx].annotation.clone();
            self.refresh_preview_image(self.show_lightbox);
        } else {
            self.select_item(target_idx);
        }
    }

    pub fn select_all_visible(&mut self) {
        let vis = self.ensure_visible_indices_cached().to_vec();
        self.selected_indices = vis.into_iter().collect();
        if self.selected_index.is_none() {
            self.selected_index = self.selected_indices.iter().next().copied();
        }
    }

    pub fn step_selection(&mut self, delta: isize) {
        let visible = self.ensure_visible_indices_cached().to_vec();
        if visible.is_empty() {
            return;
        }
        let cur_pos = self
            .selected_index
            .and_then(|sel| visible.iter().position(|&v| v == sel))
            .unwrap_or(0);
        let next_pos = (cur_pos as isize + delta).rem_euclid(visible.len() as isize) as usize;
        self.select_item(visible[next_pos]);
    }

    pub fn selected_item(&self) -> Option<&MediaItem> {
        self.selected_index.and_then(|i| self.items.get(i))
    }

    pub fn open_lightbox(&mut self) {
        if self.selected_index.is_some() {
            self.show_lightbox = true;
            self.lightbox_zoom = 0.0;
            self.refresh_preview_image(true);
        }
    }

    pub fn refresh_preview_image(&mut self, full_res: bool) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.preview_texture = None;
        self.preview_raw_image = None;
        let Some((path, is_media)) = self
            .selected_item()
            .map(|it| (it.full_path.clone(), it.is_media))
        else {
            return;
        };
        if !is_media {
            return;
        }
        let gpu_max_side = self
            .egui_ctx
            .as_ref()
            .map(|c| c.input(|i| i.max_texture_side) as u32)
            .unwrap_or(4096)
            .max(512);
        let max_preview_dim = if full_res {
            gpu_max_side.min(8192)
        } else {
            gpu_max_side.min(1600)
        };
        self.preview_worker.submit(PreviewRequest {
            generation: self.preview_generation,
            path,
            gs_config: self.settings.ghostscript.clone(),
            page: self.preview_page,
            full_res,
            max_preview_dim,
            ctx: self.egui_ctx.clone(),
        });
    }

    fn poll_preview(&mut self, ctx: &egui::Context) {
        let mut latest: Option<Option<DynamicImage>> = None;
        let mut meta_updated = false;
        for evt in self.preview_worker.drain_events() {
            if !evt.tech_meta.is_empty() {
                if let Some(&idx) = self.path_to_index.get(&evt.path) {
                    if let Some(item) = self.items.get_mut(idx) {
                        for (k, v) in evt.tech_meta {
                            item.metadata.entry(k).or_insert(v);
                        }
                        meta_updated = true;
                    }
                }
            }
            if evt.generation == self.preview_generation {
                latest = Some(evt.image);
            }
        }
        if let Some(img_opt) = latest {
            self.preview_raw_image = img_opt;
            self.rebuild_filtered_preview();
            ctx.request_repaint();
        } else if meta_updated {
            ctx.request_repaint();
        }
    }

    pub fn rebuild_filtered_preview(&mut self) {
        let Some(ref ctx) = self.egui_ctx else {
            return;
        };
        if let Some(ref raw) = self.preview_raw_image {
            let max_side = (ctx.input(|i| i.max_texture_side) as u32).max(512);
            let safe_raw = if raw.width() > max_side || raw.height() > max_side {
                let (dw, dh) =
                    crate::gpu_accel::fit_thumbnail_dims(raw.width(), raw.height(), max_side);
                std::borrow::Cow::Owned(crate::gpu_accel::fast_cpu_area_resize(raw, dw, dh))
            } else {
                std::borrow::Cow::Borrowed(raw)
            };
            let filtered = decoder::apply_filter(&safe_raw, self.active_filter);
            let rgba = filtered.to_rgba8();
            self.preview_texture = load_rgba_texture_clamped(
                ctx,
                "active_preview".to_string(),
                rgba.width(),
                rgba.height(),
                rgba.as_raw(),
                TextureOptions::LINEAR,
            );
        } else {
            self.preview_texture = None;
        }
    }

    pub fn ensure_visible_indices_cached(&mut self) -> &[usize] {
        let browse_kind = match self.browse_source {
            BrowseSource::Similar(_) => 1,
            BrowseSource::Duplicates => 2,
            _ => 0,
        };
        let loading_large_sim = self.settings.sort_field == SortField::Similarity
            && self.items.len() > 300
            && self.worker_total > 0
            && self.worker_done < self.worker_total;
        let key = VisibleCacheKey {
            items_revision: if loading_large_sim {
                self.generation
            } else {
                self.items_revision
            },
            items_structural_rev: self.items_structural_rev,
            items_len: self.items.len(),
            search_query: self.search_query.clone(),
            format_filter: self.format_filter.clone(),
            min_rating_filter: self.min_rating_filter,
            only_with_keywords: self.only_with_keywords,
            sort_field: self.settings.sort_field,
            sort_descending: self.settings.sort_descending,
            browse_kind,
            similarity_sort_epoch: self.similarity_sort_epoch,
            similarity_batch_done: !loading_large_sim,
        };
        if self.cached_visible_key.as_ref() != Some(&key) {
            self.cached_visible_indices = self.filtered_sorted_indices();
            self.cached_visible_key = Some(key);
        }
        &self.cached_visible_indices
    }

    pub fn filtered_sorted_indices(&self) -> Vec<usize> {
        let q = self.search_query.trim().to_ascii_lowercase();
        let mut indices: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                if it.rating < self.min_rating_filter {
                    return false;
                }
                if self.only_with_keywords && it.keywords.is_empty() {
                    return false;
                }
                match self.format_filter.as_str() {
                    "CDR" => matches!(it.filetype.as_str(), "CDR" | "CMX" | "CCX" | "PAT"),
                    "PSDPSB" => matches!(it.filetype.as_str(), "PSD" | "PSB"),
                    "PSD" => it.filetype == "PSD",
                    "PSB" => it.filetype == "PSB",
                    "GIF" => it.filetype == "GIF",
                    "GS" => matches!(
                        it.filetype.as_str(),
                        "PDF" | "AI" | "EPS" | "EPSI" | "EPSF" | "PS" | "PRN"
                    ),
                    "MODERN" => matches!(it.filetype.as_str(), "AVIF" | "WEBP" | "JXL"),
                    "SVG" => matches!(it.filetype.as_str(), "SVG" | "SVGZ"),
                    "RASTER" => decoder::is_common_raster_ext(it.filetype.as_str()),
                    "OTHER" => !it.is_media,
                    _ => true,
                }
            })
            .filter(|(_, it)| {
                if q.is_empty() {
                    return true;
                }
                it.file_name.to_ascii_lowercase().contains(&q)
                    || it.filetype.to_ascii_lowercase().contains(&q)
                    || it.annotation.to_ascii_lowercase().contains(&q)
                    || it
                        .keywords
                        .iter()
                        .any(|k| k.to_ascii_lowercase().contains(&q))
                    || it
                        .metadata
                        .values()
                        .any(|v| v.to_ascii_lowercase().contains(&q))
            })
            .map(|(i, _)| i)
            .collect();

        if matches!(self.browse_source, BrowseSource::Similar(_)) {
            indices.sort_by(|&a, &b| {
                let sa = self.items[a].similarity_score.unwrap_or(0.0);
                let sb = self.items[b].similarity_score.unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });
            return indices;
        }
        if matches!(self.browse_source, BrowseSource::Duplicates) {
            indices.sort_by_cached_key(|&i| {
                (
                    self.items[i].duplicate_group.unwrap_or(usize::MAX),
                    self.items[i].file_name.to_ascii_lowercase(),
                )
            });
            return indices;
        }

        let desc = self.settings.sort_descending;
        let field = self.settings.sort_field;
        if field == SortField::Name {
            indices.sort_by_cached_key(|&i| self.items[i].file_name.to_ascii_lowercase());
            if desc {
                indices.reverse();
            }
            return indices;
        }
        if field == SortField::Similarity {
            // Look up cached visual signatures from the catalog DB in O(N) RAM time,
            // falling back to in-memory thumbnail blobs for any items not yet stored with a signature.
            let paths: Vec<&Path> = indices
                .iter()
                .map(|&i| self.items[i].full_path.as_path())
                .collect();
            let mut sigs = self
                .db
                .lock()
                .ok()
                .map(|db| db.get_visual_signatures_for_paths(&paths))
                .unwrap_or_else(|| vec![None; indices.len()]);

            for (slot, &item_idx) in sigs.iter_mut().zip(indices.iter()) {
                if slot.is_none() {
                    if let Some((ref blob, ref fmt)) = self.items[item_idx].thumb_blob {
                        if let Some(img) = decoder::decode_thumbnail_blob(blob, fmt) {
                            *slot = Some(similarity::VisualSignature::from_image(&img));
                        }
                    }
                }
            }

            let anchor_idx = self
                .similarity_sort_anchor
                .as_ref()
                .and_then(|p| self.path_to_index.get(p).copied())
                .or(self.selected_index);

            let entries: Vec<(usize, Option<similarity::VisualSignature>)> =
                indices.into_iter().zip(sigs).collect();
            return similarity::order_by_visual_similarity(
                &entries,
                anchor_idx,
                self.similarity_color_weight,
                desc,
            );
        }

        indices.sort_by(|&a, &b| {
            let ia = &self.items[a];
            let ib = &self.items[b];
            let ord = match field {
                SortField::Name | SortField::Similarity => std::cmp::Ordering::Equal,
                SortField::FileType => ia.filetype.cmp(&ib.filetype),
                SortField::Size => ia.file_length.cmp(&ib.file_length),
                SortField::Modified => ia.file_time_iso.cmp(&ib.file_time_iso),
                SortField::Dimensions => {
                    (ia.width as u64 * ia.height as u64).cmp(&(ib.width as u64 * ib.height as u64))
                }
                SortField::Rating => ia.rating.cmp(&ib.rating),
                SortField::KeywordCount => ia.keywords.len().cmp(&ib.keywords.len()),
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
        indices
    }

    pub fn refresh_catalog_lists(&mut self) {
        if let Ok(db) = self.db.lock() {
            self.all_keywords = db.list_all_keywords().unwrap_or_default();
            self.all_galleries = db.list_galleries().unwrap_or_default();
            self.catalog_stats = db.stats();
            self.scanned_folders = db.scanned_folders_set();
        }
    }

    pub fn set_selected_rating(&mut self, rating: u8) {
        if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get_mut(idx) {
                item.rating = rating;
                if let Ok(mut db) = self.db.lock() {
                    let _ = db.set_rating(&item.full_path, rating);
                }
                self.bump_items_revision();
            }
        }
    }

    #[allow(dead_code)]
    pub fn add_keyword_to_selected(&mut self, kw: &str) {
        self.add_keywords_to_selected(&[kw.to_string()]);
    }

    pub fn add_keywords_to_selected(&mut self, kws: &[String]) {
        let cleaned: Vec<String> = kws
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if cleaned.is_empty() {
            return;
        }
        let Some(idx) = self.selected_index else {
            return;
        };
        let mut sync_target: Option<(PathBuf, String, Vec<String>)> = None;
        if let Some(item) = self.items.get_mut(idx) {
            let mut added = Vec::new();
            for kw in &cleaned {
                if !item.keywords.iter().any(|k| k.eq_ignore_ascii_case(kw)) {
                    item.keywords.push(kw.clone());
                    added.push(kw.clone());
                }
            }
            if !added.is_empty() {
                item.keywords.sort_by_key(|a| a.to_ascii_lowercase());
            }
            if let Ok(mut db) = self.db.lock() {
                let _ = db.add_keywords_to_path(&item.full_path, &cleaned);
            }
            sync_target = Some((
                item.full_path.clone(),
                item.file_name.clone(),
                item.keywords.clone(),
            ));
        }
        if let Some((path, fname, all_kws)) = sync_target {
            self.bump_items_revision();
            self.sync_keywords_to_file_if_enabled(idx, &path, &fname, &all_kws);
        }
        self.refresh_catalog_lists();
    }

    pub fn remove_keyword_from_selected(&mut self, kw: &str) {
        let Some(idx) = self.selected_index else {
            return;
        };
        let mut sync_target: Option<(PathBuf, String, Vec<String>)> = None;
        if let Some(item) = self.items.get_mut(idx) {
            item.keywords.retain(|k| !k.eq_ignore_ascii_case(kw));
            if let Ok(mut db) = self.db.lock() {
                let _ = db.remove_keyword_from_path(&item.full_path, kw);
            }
            sync_target = Some((
                item.full_path.clone(),
                item.file_name.clone(),
                item.keywords.clone(),
            ));
        }
        if let Some((path, fname, all_kws)) = sync_target {
            self.bump_items_revision();
            self.sync_keywords_to_file_if_enabled(idx, &path, &fname, &all_kws);
        }
        self.refresh_catalog_lists();
    }

    fn sync_keywords_to_file_if_enabled(
        &mut self,
        idx: usize,
        path: &Path,
        fname: &str,
        keywords: &[String],
    ) {
        if !self.settings.write_keywords_to_files {
            return;
        }
        match keyword_writer::write_keywords_to_file(path, keywords) {
            Ok(outcome) => {
                let (file_length, file_time_iso) = worker::file_stat(path);
                let md5 = similarity::md5_file_capped(path, MAX_MD5_FILE_KB * 1024)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if let Some(item) = self.items.get_mut(idx) {
                    if item.full_path == path {
                        item.file_length = file_length;
                        item.file_time_iso = file_time_iso.clone();
                        item.md5 = md5.clone();
                    }
                }
                if let Ok(mut db) = self.db.lock() {
                    let _ = db.update_file_stat(path, file_length, &file_time_iso, &md5);
                }
                let msg = format!(
                    "Synced {} tag(s) to {fname} ({})",
                    keywords.len(),
                    outcome.method
                );
                self.keyword_sync_status = Some((true, msg.clone()));
                self.push_log(format!("{msg} for Windows Explorer"));
            }
            Err(err) => {
                let msg = format!("DB only for {fname}: {err}");
                self.keyword_sync_status = Some((false, msg.clone()));
                self.push_log(format!("Saved keyword(s) to database only for {fname}: {err}"));
            }
        }
    }

    pub fn save_selected_annotation(&mut self) {
        if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get_mut(idx) {
                item.annotation = self.annotation_draft.clone();
                if let Ok(mut db) = self.db.lock() {
                    let _ = db.set_annotation(&item.full_path, &item.annotation);
                }
                let fname = item.file_name.clone();
                self.bump_items_revision();
                self.push_log(format!("Saved annotation for {fname}"));
            }
        }
    }

    pub fn create_gallery(&mut self, name: &str) {
        if let Ok(mut db) = self.db.lock() {
            let _ = db.create_gallery(name);
        }
        self.refresh_catalog_lists();
    }

    pub fn delete_gallery(&mut self, name: &str) {
        if let Ok(mut db) = self.db.lock() {
            let _ = db.delete_gallery(name);
        }
        self.refresh_catalog_lists();
    }

    pub fn add_selected_to_gallery(&mut self, gallery: &str) {
        if let Some(item) = self.selected_item().cloned() {
            if let Ok(mut db) = self.db.lock() {
                let _ = db.add_path_to_gallery(gallery, &item.full_path);
            }
            self.push_log(format!("Added {} to gallery '{}'", item.file_name, gallery));
            self.refresh_catalog_lists();
        }
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let events = self.worker.drain_events();
        let mut updated_any = false;
        let mut need_initial_preview = false;

        for evt in events {
            match evt {
                WorkerEvent::Progress {
                    generation,
                    done,
                    total,
                    current_file,
                } => {
                    if generation == self.generation {
                        self.worker_done = done;
                        self.worker_total = total;
                        self.worker_current_file = current_file;
                    }
                }
                WorkerEvent::Log(line) => {
                    self.push_log(line);
                }
                WorkerEvent::ItemReady {
                    generation,
                    path,
                    filetype,
                    width,
                    height,
                    color_depth,
                    page_count,
                    file_length,
                    file_time_iso,
                    rating,
                    annotation,
                    keywords,
                    metadata,
                    rgba,
                    thumb_blob,
                    md5,
                    source_note,
                } => {
                    if generation != self.generation {
                        continue;
                    }
                    if let Some(&idx) = self.path_to_index.get(&path) {
                        if let Some(item) = self.items.get_mut(idx) {
                            item.filetype = filetype;
                            item.width = width;
                            item.height = height;
                            item.color_depth = color_depth;
                            item.page_count = page_count;
                            item.file_length = file_length;
                            item.file_time_iso = file_time_iso;
                            item.rating = rating;
                            item.annotation = annotation;
                            item.keywords = keywords;
                            item.metadata = metadata;
                            item.md5 = md5;
                            item.source_note = source_note;
                            item.loaded = true;
                            item.thumb_blob = thumb_blob.map(|(b, f)| (Arc::new(b), f));
                            self.pending_tex_decodes.remove(&idx);
                            if let Some((w, h, raw)) = rgba {
                                if let Some(tex) = load_rgba_texture_clamped(
                                    ctx,
                                    format!("thumb_{}_{}", generation, idx),
                                    w,
                                    h,
                                    &raw,
                                    TextureOptions::LINEAR,
                                ) {
                                    if item.texture.is_none() {
                                        self.live_texture_count += 1;
                                    }
                                    item.texture = Some(tex);
                                    item.last_drawn_frame = self.ui_frame;
                                }
                            }
                            if self.selected_index == Some(idx) && self.preview_texture.is_none() {
                                self.annotation_draft = item.annotation.clone();
                                need_initial_preview = true;
                            }
                            updated_any = true;
                            self.worker_items_since_refresh += 1;
                        }
                    }
                }
                WorkerEvent::BatchDone { generation, total } => {
                    if generation == self.generation {
                        self.worker_done = total;
                        self.worker_current_file.clear();
                        self.worker_items_since_refresh = 0;
                        if let BrowseSource::Folder(ref dir) = self.browse_source {
                            let mut dirs = vec![dir.clone()];
                            if self.settings.include_subfolders {
                                let mut seen = HashSet::new();
                                seen.insert(dir.clone());
                                for it in &self.items {
                                    if let Some(p) = it.full_path.parent() {
                                        let pb = p.to_path_buf();
                                        if seen.insert(pb.clone()) {
                                            dirs.push(pb);
                                        }
                                    }
                                }
                            }
                            if let Ok(mut db) = self.db.lock() {
                                let _ = db.mark_folders_scanned(&dirs);
                            }
                        }
                        self.push_log(format!("Completed indexing {total} file(s)."));
                        self.refresh_catalog_lists();
                    }
                }
            }
        }

        if need_initial_preview {
            self.refresh_preview_image(false);
        }
        if self.worker_items_since_refresh >= 200 {
            self.worker_items_since_refresh = 0;
            self.refresh_catalog_lists();
        }
        if updated_any {
            self.bump_items_revision();
            ctx.request_repaint();
        }
        if self.worker_total > 0 && self.worker_done < self.worker_total {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
    }

    fn poll_scanner(&mut self, ctx: &egui::Context) {
        let events = self.scanner.drain_events();
        let mut any = false;
        for evt in events {
            any = true;
            match evt {
                ScanEvent::Started {
                    task_id,
                    started_at,
                    thread_id,
                    total,
                    seen,
                } => {
                    if let Some(t) = self.managed_tasks.iter_mut().find(|t| t.id == task_id) {
                        t.started_at = started_at;
                        t.thread_id = thread_id;
                        t.total = total;
                        t.seen = seen;
                        t.state = TaskState::Running;
                    }
                }
                ScanEvent::Progress {
                    task_id,
                    done,
                    total,
                    seen,
                    indexed,
                    skipped,
                    current_file,
                    elapsed_ms,
                    active_ms,
                    state,
                } => {
                    if let Some(t) = self.managed_tasks.iter_mut().find(|t| t.id == task_id) {
                        t.done = done;
                        t.total = total;
                        t.seen = seen;
                        t.indexed = indexed;
                        t.skipped = skipped;
                        t.current_file = current_file;
                        t.elapsed_ms = elapsed_ms;
                        t.active_ms = active_ms;
                        t.state = state;
                    }
                    if done > 0 && done % 200 == 0 {
                        self.refresh_catalog_lists();
                    }
                }
                ScanEvent::ItemRefreshed {
                    path,
                    filetype,
                    width,
                    height,
                    color_depth,
                    page_count,
                    file_length,
                    file_time_iso,
                    keywords,
                    metadata,
                    thumb_blob,
                    md5,
                    source_note,
                } => {
                    if let Some(&idx) = self.path_to_index.get(&path) {
                        if let Some(item) = self.items.get_mut(idx) {
                            item.filetype = filetype;
                            item.width = width;
                            item.height = height;
                            item.color_depth = color_depth;
                            item.page_count = page_count;
                            item.file_length = file_length;
                            item.file_time_iso = file_time_iso;
                            item.keywords = keywords;
                            item.metadata = metadata;
                            item.md5 = md5;
                            item.source_note = source_note;
                            item.loaded = true;
                            item.thumb_blob = thumb_blob.map(|(b, f)| (Arc::new(b), f));
                            if item.texture.take().is_some() {
                                self.live_texture_count = self.live_texture_count.saturating_sub(1);
                            }
                            self.pending_tex_decodes.remove(&idx);
                            self.request_item_texture_async(ctx, idx);
                            self.bump_items_revision();
                        }
                        if self.selected_index == Some(idx) {
                            self.refresh_preview_image(self.show_lightbox);
                        }
                    }
                }
                ScanEvent::TaskMessage {
                    task_id,
                    message,
                    is_error,
                } => {
                    if let Some(t) = self.managed_tasks.iter_mut().find(|t| t.id == task_id) {
                        if is_error {
                            t.errors.push(message);
                            if t.errors.len() > 200 {
                                t.errors.remove(0);
                            }
                        } else {
                            t.messages.push(message);
                            if t.messages.len() > 200 {
                                t.messages.remove(0);
                            }
                        }
                    }
                }
                ScanEvent::Log(line) => {
                    self.push_log(line);
                }
                ScanEvent::Finished {
                    task_id,
                    completed_at,
                    elapsed_ms,
                    active_ms,
                    done,
                    total,
                    seen,
                    indexed,
                    skipped,
                    cancelled,
                    scanned_dirs,
                } => {
                    let mut summary = String::new();
                    if let Some(t) = self.managed_tasks.iter_mut().find(|t| t.id == task_id) {
                        t.completed_at = completed_at;
                        t.elapsed_ms = elapsed_ms;
                        t.active_ms = active_ms;
                        t.done = done;
                        t.total = total;
                        t.seen = seen;
                        t.indexed = indexed;
                        t.skipped = skipped;
                        t.current_file.clear();
                        t.state = if cancelled {
                            TaskState::Cancelled
                        } else {
                            TaskState::Completed
                        };
                        let status = if cancelled { "cancelled" } else { "finished" };
                        summary = format!(
                            "{} {status}: {indexed} indexed, {skipped} up-to-date.",
                            t.display_text
                        );
                        t.messages.push(summary.clone());
                    }
                    if !cancelled {
                        for d in &scanned_dirs {
                            let key = crate::catalog::normalize_dir_key(d);
                            if !key.is_empty() {
                                self.scanned_folders.insert(key);
                            }
                        }
                    }
                    if !summary.is_empty() {
                        self.push_log(summary);
                    }
                    if self.settings.auto_clear_completed_tasks && !cancelled {
                        if self.task_props_id != Some(task_id) {
                            self.managed_tasks.retain(|t| t.id != task_id);
                        }
                    }
                    self.refresh_catalog_lists();
                }
            }
        }
        let has_active = self.managed_tasks.iter().any(|t| t.state.is_active());
        if any || has_active {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
    }

    fn poll_folder_watcher(&mut self, ctx: &egui::Context) {
        let desired = match &self.browse_source {
            BrowseSource::Folder(dir) => Some((dir.clone(), self.settings.include_subfolders)),
            _ => None,
        };
        self.folder_watcher.set_target(desired, Some(ctx.clone()));
        if self.folder_watcher.take_dirty() {
            self.sync_active_folder_incremental(ctx);
        }
    }

    /// Incrementally updates the active folder view when files are added, deleted, or modified on disk.
    /// Keeps existing loaded thumbnails and current selection intact to avoid flicker.
    fn sync_active_folder_incremental(&mut self, ctx: &egui::Context) {
        let dir = match &self.browse_source {
            BrowseSource::Folder(d) => d.clone(),
            _ => return,
        };
        if !dir.is_dir() {
            return;
        }

        let disk_files = collect_folder_files(&dir, self.settings.include_subfolders);
        let disk_set: HashSet<PathBuf> = disk_files.iter().cloned().collect();

        // Preserve selection by path across incremental refresh
        let selected_paths: HashSet<PathBuf> = self
            .selected_indices
            .iter()
            .filter_map(|&i| self.items.get(i).map(|it| it.full_path.clone()))
            .collect();
        let primary_path = self
            .selected_index
            .and_then(|i| self.items.get(i).map(|it| it.full_path.clone()));

        let mut added: Vec<PathBuf> = Vec::new();
        for p in &disk_files {
            if !self.path_to_index.contains_key(p) {
                added.push(p.clone());
            }
        }

        let mut removed_count = 0usize;
        let mut modified: Vec<PathBuf> = Vec::new();
        let mut primary_was_modified = false;

        let mut kept_items = Vec::with_capacity(disk_files.len());
        for mut item in std::mem::take(&mut self.items) {
            if !disk_set.contains(&item.full_path) {
                removed_count += 1;
                continue;
            }
            let (new_len, new_mtime) = crate::worker::file_stat(&item.full_path);
            if new_len != item.file_length || new_mtime != item.file_time_iso {
                item.file_length = new_len;
                item.file_time_iso = new_mtime;
                if item.is_media {
                    item.loaded = false;
                    item.thumb_blob = None;
                    item.texture = None;
                    item.source_note = "Updating...".to_string();
                    if primary_path.as_ref() == Some(&item.full_path) {
                        primary_was_modified = true;
                    }
                    modified.push(item.full_path.clone());
                } else {
                    let key = crate::shell_icon::icon_cache_key(&item.full_path);
                    if key.starts_with("path:") {
                        self.native_icon_cache.remove(&key);
                        item.texture = None;
                    }
                }
            }
            kept_items.push(item);
        }

        if added.is_empty() && removed_count == 0 && modified.is_empty() {
            self.items = kept_items;
            return;
        }

        for p in &added {
            let is_media = decoder::is_supported_file(p);
            let fname = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_uppercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "FILE".to_string());
            let (file_length, file_time_iso) = crate::worker::file_stat(p);
            kept_items.push(MediaItem {
                full_path: p.clone(),
                file_name: fname,
                filetype: ext,
                is_media,
                width: 0,
                height: 0,
                color_depth: if is_media { 24 } else { 0 },
                page_count: if is_media { 1 } else { 0 },
                file_length,
                file_time_iso,
                rating: 0,
                annotation: String::new(),
                keywords: Vec::new(),
                metadata: BTreeMap::new(),
                md5: String::new(),
                similarity_score: None,
                duplicate_group: None,
                source_note: if is_media {
                    "Queued".to_string()
                } else {
                    "Native File Icon".to_string()
                },
                loaded: !is_media,
                thumb_blob: None,
                texture: None,
                last_drawn_frame: 0,
            });
        }

        self.items = kept_items;
        self.pending_tex_decodes.clear();
        self.invalidate_dir_tree_cache(Some(&dir));
        self.bump_items_structural_revision();
        self.path_to_index.clear();
        self.live_texture_count = 0;
        for (i, it) in self.items.iter().enumerate() {
            self.path_to_index.insert(it.full_path.clone(), i);
            if it.is_media && it.texture.is_some() {
                self.live_texture_count += 1;
            }
        }

        // Restore selection
        self.selected_indices.clear();
        for p in &selected_paths {
            if let Some(&i) = self.path_to_index.get(p) {
                self.selected_indices.insert(i);
            }
        }
        self.selected_index = primary_path
            .as_ref()
            .and_then(|p| self.path_to_index.get(p).copied())
            .or_else(|| self.selected_indices.iter().next().copied())
            .or_else(|| if self.items.is_empty() { None } else { Some(0) });
        if let Some(idx) = self.selected_index {
            self.selected_indices.insert(idx);
        } else {
            self.preview_texture = None;
            self.preview_raw_image = None;
        }

        // Queue added, modified, and any still-unloaded media items so nothing is left stuck on "Loading..."
        let mut to_load: Vec<PathBuf> = Vec::new();
        let mut seen_load: HashSet<PathBuf> = HashSet::new();
        for p in added.iter().chain(modified.iter()) {
            if decoder::is_supported_file(p) && seen_load.insert(p.clone()) {
                to_load.push(p.clone());
            }
        }
        for it in &self.items {
            if it.is_media && !it.loaded && seen_load.insert(it.full_path.clone()) {
                to_load.push(it.full_path.clone());
            }
        }

        if !to_load.is_empty() {
            self.generation += 1;
            self.worker_done = 0;
            self.worker_total = to_load.len();
            self.worker_current_file.clear();
            let send_blob_only = self.items.len() > 400;
            self.worker.submit(WorkerRequest {
                files: to_load,
                gs_config: self.settings.ghostscript.clone(),
                thumb_cfg: self.thumb_encode_cfg(),
                db: self.db.clone(),
                force_redecode: false,
                generation: self.generation,
                send_blob_only,
                cache_only: !self.settings.auto_scan,
            });
        }

        if primary_was_modified {
            self.refresh_preview_image(self.show_lightbox);
        }

        let mut parts = Vec::new();
        if !added.is_empty() {
            parts.push(format!("+{} added", added.len()));
        }
        if removed_count > 0 {
            parts.push(format!("-{removed_count} removed"));
        }
        if !modified.is_empty() {
            parts.push(format!("~{} updated", modified.len()));
        }
        if !parts.is_empty() {
            self.push_log(format!("Auto-updated folder ({})", parts.join(", ")));
        }
        ctx.request_repaint();
    }

    fn handle_internal_drag_overlay_and_handoff(
        &mut self,
        ctx: &egui::Context,
        pal: &theme::ThemePalette,
    ) {
        if self.internal_drag_files.is_empty() {
            return;
        }
        let (egui_primary_down, esc, alt_mod, focused, cur_pos) = ctx.input(|i| {
            (
                i.pointer.primary_down(),
                i.key_pressed(egui::Key::Escape),
                i.modifiers.alt,
                i.viewport().focused.unwrap_or(true),
                i.pointer.interact_pos(),
            )
        });
        let os_primary_down = drag_drop::is_primary_mouse_button_down();
        let switch_mod = alt_mod || drag_drop::is_switch_modifier_down();

        if esc {
            self.internal_drag_files.clear();
            self.internal_drag_hover_dir = None;
            return;
        }

        // Only treat the drag as released if BOTH egui and the OS agree the button is up.
        // When the user presses Alt+Tab, winit resets egui's pointer state on focus loss even
        // while the user is still physically holding the mouse button.
        if !egui_primary_down && !os_primary_down {
            let files = std::mem::take(&mut self.internal_drag_files);
            let target = self.internal_drag_hover_dir.take();
            if let Some(dest_dir) = target {
                self.move_files_to_folder(&files, &dest_dir);
            }
            return;
        }

        // Hand off seamlessly to native Windows OLE DoDragDrop whenever:
        // 1. The cursor leaves ThumbCraft's window, OR
        // 2. The user presses Alt / Win (e.g. Alt+Tab, Win+Tab, Win+D) — hands off immediately
        //    while ThumbCraft is still foreground so Windows' OLE drag loop survives Alt+Tab, OR
        // 3. ThumbCraft lost focus while the physical mouse button is still held down.
        let screen = ctx.screen_rect();
        let outside_egui = cur_pos
            .map(|p| !screen.contains(p))
            .unwrap_or(false);
        let lost_focus_while_dragging = !focused || !egui_primary_down;
        if drag_drop::is_cursor_outside_active_window()
            || outside_egui
            || switch_mod
            || lost_focus_while_dragging
        {
            let files = std::mem::take(&mut self.internal_drag_files);
            self.internal_drag_hover_dir = None;
            self.drag_candidate = None;
            self.start_external_drag_paths(files);
            return;
        }

        // Draw floating drag badge near cursor while dragging inside ThumbCraft
        if let Some(pos) = cur_pos {
            let count = self.internal_drag_files.len();
            let item_lbl = if count == 1 {
                self.internal_drag_files[0]
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("1 file")
                    .to_string()
            } else {
                format!("{count} files")
            };
            let (badge_text, border_col) = if let Some(ref dest) = self.internal_drag_hover_dir {
                let dir_name = dest
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| dest.to_string_lossy().to_string());
                (
                    format!("Move {item_lbl} -> {dir_name}"),
                    egui::Color32::from_rgb(34, 197, 94),
                )
            } else {
                (
                    format!("Dragging {item_lbl} (drop on folder, drag outside, or Alt+Tab)"),
                    pal.accent,
                )
            };

            egui::Area::new(egui::Id::new("internal_drag_ghost"))
                .order(egui::Order::Tooltip)
                .interactable(false)
                .fixed_pos(pos + egui::vec2(14.0, 14.0))
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(pal.bg_chrome_dark)
                        .stroke(egui::Stroke::new(1.5_f32, border_col))
                        .corner_radius(egui::CornerRadius::same(5))
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(badge_text)
                                    .size(11.0)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            );
                        });
                });
        }
        // Keep polling every frame during an active drag so Alt+Tab is caught immediately
        // even if the mouse pointer is held stationary.
        ctx.request_repaint();
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(ref target_path) = self.screenshot_target.clone() else {
            return;
        };

        self.frame_count = self.frame_count.saturating_add(1);
        ctx.request_repaint();

        let captured = ctx.input(|i| {
            i.events.iter().find_map(|ev| {
                if let egui::Event::Screenshot { image, .. } = ev {
                    Some(image.clone())
                } else {
                    None
                }
            })
        });

        if let Some(color_image) = captured {
            let w = color_image.width() as u32;
            let h = color_image.height() as u32;
            let mut raw = Vec::with_capacity((w * h * 4) as usize);
            for p in &color_image.pixels {
                raw.extend_from_slice(&[p.r(), p.g(), p.b(), p.a()]);
            }
            if let Some(parent) = target_path.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
            if let Some(buf) = image::RgbaImage::from_raw(w, h, raw) {
                let _ = buf.save(target_path);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let indexing_done = self.worker_total == 0 || self.worker_done >= self.worker_total;
        if !self.screenshot_requested
            && ((self.frame_count >= 120 && indexing_done) || self.frame_count >= 600)
        {
            self.screenshot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }
}

impl eframe::App for ThumbCraftApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui_frame = self.ui_frame.saturating_add(1);
        self.egui_ctx = Some(ctx.clone());
        self.internal_drag_hover_dir = None;
        let pal = if self.applied_theme != Some(self.settings.theme) {
            let p = theme::apply_theme(ctx, self.settings.theme);
            self.applied_theme = Some(self.settings.theme);
            self.cached_palette = p;
            p
        } else {
            self.cached_palette
        };
        self.poll_worker(ctx);
        self.poll_scanner(ctx);
        self.poll_preview(ctx);
        self.poll_thumb_decodes(ctx);
        self.poll_folder_watcher(ctx);

        // 1. TOP MENU BAR
        egui::TopBottomPanel::top("menu_bar_panel")
            .exact_height(26.0)
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg_chrome_dark)
                    .inner_margin(egui::Margin::symmetric(6, 2)),
            )
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(egui::Color32::from_rgb(242, 244, 248));
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        ui.visuals_mut().override_text_color = None;
                        if ui.button("Open Folder...").clicked() {
                            ui.close_menu();
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                self.open_folder(dir);
                            }
                        }
                        if ui.button("Open Built-in Samples").clicked() {
                            ui.close_menu();
                            let d = self.sample_dir.clone();
                            self.open_folder(d);
                        }
                        if ui.button("Rebuild Selected Thumbnail(s) (F3)").clicked() {
                            ui.close_menu();
                            self.rebuild_selected_thumbnails();
                        }
                        if ui.button("Refresh & Re-scan Folder").clicked() {
                            ui.close_menu();
                            self.reload_current_source(true);
                        }
                        ui.separator();
                        if ui.button("Preferences & Ghostscript...").clicked() {
                            ui.close_menu();
                            self.show_preferences = true;
                        }
                        ui.separator();
                        if ui.button("Exit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    ui.menu_button("Database", |ui| {
                        ui.visuals_mut().override_text_color = None;
                        if ui.button("Scan Current Folder (Shift+Alt+F3)").clicked() {
                            self.start_folder_scan(self.last_folder_context.clone(), false, false);
                            ui.close_menu();
                        }
                        if ui.button("Scan Current Tree (Ctrl+Shift+F3)").clicked() {
                            self.start_folder_scan(self.last_folder_context.clone(), true, false);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Find Similar Images... (Alt+0)").clicked() {
                            if let Some(item) = self.selected_item().cloned() {
                                self.open_similar_dialog(item.full_path);
                            }
                            ui.close_menu();
                        }
                        if ui
                            .button("Find Duplicate Files (Scan MD5 Hashes)")
                            .clicked()
                        {
                            self.open_duplicates_view();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Database Location & Settings...").clicked() {
                            self.show_preferences = true;
                            self.prefs_tab = PrefsTab::Thumbnails;
                            ui.close_menu();
                        }
                        if ui.button("Open Database Location in Explorer").clicked() {
                            self.open_db_in_explorer();
                            ui.close_menu();
                        }
                        if ui.button("Compact Database").clicked() {
                            self.compact_database();
                            ui.close_menu();
                        }
                        if ui.button("Remove All Orphans...").clicked() {
                            self.remove_folder_orphans(None);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Browse All Keywords").clicked() {
                            self.settings.layout.show_tree_pane = true;
                            self.tree_tab = TreeTab::Keywords;
                            ui.close_menu();
                        }
                        if ui.button("Browse Galleries").clicked() {
                            self.settings.layout.show_tree_pane = true;
                            self.tree_tab = TreeTab::Galleries;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .checkbox(&mut self.settings.auto_scan, "Auto-Scan Folders")
                            .on_hover_text("Automatically decode and catalog uncached thumbnails when opening folders")
                            .clicked()
                        {
                            self.settings.save();
                            if self.settings.auto_scan {
                                self.reload_current_source(false);
                            }
                        }
                        if ui
                            .checkbox(&mut self.settings.include_subfolders, "Include Subfolders")
                            .clicked()
                        {
                            self.settings.save();
                            self.reload_current_source(false);
                        }
                    });

                    ui.menu_button("View", |ui| {
                        ui.visuals_mut().override_text_color = None;
                        ui.label(RichText::new("View Mode").strong().size(11.0));
                        for (mode, label) in [
                            (ViewMode::Thumbnails, "Thumbnail Grid"),
                            (ViewMode::Details, "Details Table"),
                            (ViewMode::Filmstrip, "Filmstrip"),
                        ] {
                            if ui
                                .selectable_value(&mut self.settings.view_mode, mode, label)
                                .clicked()
                            {
                                self.settings.save();
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        ui.label(RichText::new("Layout Presets").strong().size(11.0));
                        for preset in LayoutPreset::all() {
                            if ui.button(preset.label()).clicked() {
                                self.settings.apply_preset(*preset);
                                self.settings.save();
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        ui.label(RichText::new("Panes").strong().size(11.0));
                        ui.checkbox(
                            &mut self.settings.layout.show_tree_pane,
                            "Location / Tree Pane",
                        );
                        ui.checkbox(&mut self.settings.layout.show_preview_pane, "Preview Pane");
                        ui.checkbox(
                            &mut self.settings.layout.show_info_pane,
                            "Metadata & Keywords Pane",
                        );
                        ui.checkbox(
                            &mut self.settings.layout.show_tasks_pane,
                            "Tasks & Ghostscript Pane",
                        );
                        ui.checkbox(&mut self.settings.layout.show_toolbar, "Toolbar");
                        ui.checkbox(&mut self.settings.layout.show_status_bar, "Status Bar");
                    });

                    ui.menu_button("Theme", |ui| {
                        ui.visuals_mut().override_text_color = None;
                        for t in AppTheme::all() {
                            if ui
                                .selectable_value(&mut self.settings.theme, *t, t.label())
                                .clicked()
                            {
                                self.settings.save();
                                ui.close_menu();
                            }
                        }
                    });

                    ui.menu_button("Options", |ui| {
                        ui.visuals_mut().override_text_color = None;
                        if ui.button("Preferences & UI Customization...").clicked() {
                            self.show_preferences = true;
                            self.prefs_tab = PrefsTab::Layout;
                            ui.close_menu();
                        }
                        if ui.button("Ghostscript PS/EPS/PDF & CDR...").clicked() {
                            self.show_preferences = true;
                            self.prefs_tab = PrefsTab::Ghostscript;
                            ui.close_menu();
                        }
                        if ui.button("Thumbnails, Database & Captions...").clicked() {
                            self.show_preferences = true;
                            self.prefs_tab = PrefsTab::Thumbnails;
                            ui.close_menu();
                        }
                        if ui.button("Task Preferences...").clicked() {
                            self.show_preferences = true;
                            self.prefs_tab = PrefsTab::Tasks;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Re-render Folder with Ghostscript").clicked() {
                            self.reload_current_source(true);
                            ui.close_menu();
                        }
                    });
                });
            });

        // 2. MAIN TOOLBAR
        if self.settings.layout.show_toolbar {
            egui::TopBottomPanel::top("toolbar_panel")
                .exact_height(38.0)
                .frame(
                    egui::Frame::NONE
                        .fill(pal.bg_toolbar)
                        .inner_margin(egui::Margin::symmetric(8, 5)),
                )
                .show(ctx, |ui| {
                    ui.horizontal_centered(|ui| {
                        if ui
                            .add_enabled(!self.history_back.is_empty(), egui::Button::new("◀"))
                            .on_hover_text("Back")
                            .clicked()
                        {
                            self.go_back();
                        }
                        if ui
                            .add_enabled(!self.history_fwd.is_empty(), egui::Button::new("▶"))
                            .on_hover_text("Forward")
                            .clicked()
                        {
                            self.go_forward();
                        }
                        if ui.button("Up").on_hover_text("Parent folder").clicked() {
                            self.go_up();
                        }
                        if ui.button("Open Folder").on_hover_text("Choose folder").clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                self.open_folder(dir);
                            }
                        }
                        if ui
                            .button("Samples")
                            .on_hover_text("Built-in CDR/GS/AVIF samples")
                            .clicked()
                        {
                            let d = self.sample_dir.clone();
                            self.open_folder(d);
                        }
                        if ui
                            .button("Similar")
                            .on_hover_text(
                                "Find visually similar images (Entire DB / Current Tree / Current Folder, Alt+0)",
                            )
                            .clicked()
                        {
                            if let Some(item) = self.selected_item().cloned() {
                                self.open_similar_dialog(item.full_path);
                            }
                        }
                        if ui
                            .button("Duplicates")
                            .on_hover_text(
                                "Scan MD5 hashes for exact duplicates (Entire DB / Current Tree / Current Folder)",
                            )
                            .clicked()
                        {
                            self.open_duplicates_view();
                        }

                        let auto_lbl = if self.settings.auto_scan {
                            "Auto-Scan: ON"
                        } else {
                            "Auto-Scan: OFF"
                        };
                        let auto_btn = egui::Button::new(auto_lbl)
                            .selected(self.settings.auto_scan);
                        if ui
                            .add(auto_btn)
                            .on_hover_text(
                                "Toggle automatic thumbnail scanning when browsing folders (already-cached DB thumbnails always load)",
                            )
                            .clicked()
                        {
                            self.settings.auto_scan = !self.settings.auto_scan;
                            self.settings.save();
                            self.push_log(format!(
                                "Auto-Scan turned {}.",
                                if self.settings.auto_scan { "ON" } else { "OFF" }
                            ));
                            if self.settings.auto_scan {
                                self.reload_current_source(false);
                            }
                        }

                        ui.separator();

                        let path_edit = ui.add(
                            egui::TextEdit::singleline(&mut self.path_bar_input)
                                .desired_width(195.0)
                                .hint_text("Folder path..."),
                        );
                        if path_edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let p = PathBuf::from(self.path_bar_input.trim());
                            if p.is_dir() {
                                self.open_folder(p);
                            }
                        }

                        ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .desired_width(165.0)
                                .hint_text("Search name or CDR keyword..."),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Customize UI").clicked() {
                                self.show_preferences = true;
                                self.prefs_tab = PrefsTab::Layout;
                            }
                            if ui.button("Ghostscript").clicked() {
                                self.show_preferences = true;
                                self.prefs_tab = PrefsTab::Ghostscript;
                            }
                            egui::ComboBox::from_id_salt("toolbar_theme")
                                .width(108.0)
                                .selected_text(self.settings.theme.label())
                                .show_ui(ui, |ui| {
                                    for t in AppTheme::all() {
                                        if ui
                                            .selectable_value(
                                                &mut self.settings.theme,
                                                *t,
                                                t.label(),
                                            )
                                            .clicked()
                                        {
                                            self.settings.save();
                                        }
                                    }
                                });
                            egui::ComboBox::from_id_salt("toolbar_preset")
                                .width(100.0)
                                .selected_text("Layout")
                                .show_ui(ui, |ui| {
                                    for p in LayoutPreset::all() {
                                        if ui.button(p.label()).clicked() {
                                            self.settings.apply_preset(*p);
                                            self.settings.save();
                                        }
                                    }
                                });
                        });
                    });
                });
        }

        // 3. STATUS BAR
        if self.settings.layout.show_status_bar {
            egui::TopBottomPanel::bottom("status_bar_panel")
                .exact_height(24.0)
                .frame(
                    egui::Frame::NONE
                        .fill(pal.bg_chrome_dark)
                        .inner_margin(egui::Margin::symmetric(8, 3)),
                )
                .show(ctx, |ui| {
                    let st_txt = egui::Color32::from_rgb(236, 238, 242);
                    ui.horizontal_centered(|ui| {
                        ui.label(
                            RichText::new(format!("{} files", self.items.len()))
                                .size(11.0)
                                .color(st_txt),
                        );
                        ui.separator();
                        let sel_cnt = self.selected_indices.len();
                        if sel_cnt > 1 {
                            ui.label(
                                RichText::new(format!("{sel_cnt} selected (F3: Rebuild Selected)"))
                                    .size(11.0)
                                    .color(st_txt),
                            );
                        } else if let Some(item) = self.selected_item() {
                            let status_text = format!(
                                "1 selected: {} ({}, {}×{}, {} keyword{})",
                                item.full_path.display(),
                                item.filetype,
                                item.width,
                                item.height,
                                item.keywords.len(),
                                if item.keywords.len() == 1 { "" } else { "s" }
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&status_text).size(11.0).color(st_txt),
                                )
                                .truncate(),
                            )
                            .on_hover_text(status_text);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let gs_lbl = match &self.gs_version {
                                Some(v) => format!("GS {v}"),
                                None => "GS Missing".to_string(),
                            };
                            ui.label(
                                RichText::new(format!(
                                    "DB: {} thumbs ({}) • {} Q{} • {} keywords • {}",
                                    self.catalog_stats.total_thumbnails,
                                    thumb_pane::format_bytes(self.catalog_stats.db_size_bytes),
                                    self.settings.thumb_format.short_name(),
                                    self.settings.thumb_quality,
                                    self.catalog_stats.total_keywords,
                                    gs_lbl
                                ))
                                .size(11.0)
                                .color(st_txt),
                            );
                        });
                    });
                });
        }

        let dock_frame = egui::Frame::NONE
            .fill(pal.bg_panel)
            .stroke(egui::Stroke::new(1.0_f32, pal.border_subtle))
            .inner_margin(egui::Margin::same(2));

        // 4. LEFT SIDEBAR: TREE PANE + (OPTIONAL) PREVIEW PANE
        let show_left = self.settings.layout.show_tree_pane
            || (self.settings.layout.show_preview_pane && !self.settings.layout.preview_on_right);
        if show_left {
            egui::SidePanel::left("left_dock_panel")
                .frame(dock_frame)
                .resizable(true)
                .default_width(self.settings.layout.tree_width)
                .min_width(180.0)
                .max_width(420.0)
                .show(ctx, |ui| {
                    let show_prev_here = self.settings.layout.show_preview_pane
                        && !self.settings.layout.preview_on_right;
                    if self.settings.layout.show_tree_pane && show_prev_here {
                        let prev_h = self.settings.layout.preview_height.clamp(150.0, 360.0);
                        let tree_h = (ui.available_height() - prev_h - 6.0).max(160.0);
                        ui.allocate_ui(egui::vec2(ui.available_width(), tree_h), |ui| {
                            tree_pane::show_tree_pane(ui, self, &pal);
                        });
                        ui.add_space(2.0);
                        preview_pane::show_preview_pane(ui, self, &pal);
                    } else if self.settings.layout.show_tree_pane {
                        tree_pane::show_tree_pane(ui, self, &pal);
                    } else if show_prev_here {
                        preview_pane::show_preview_pane(ui, self, &pal);
                    }
                });
        }

        // 5. RIGHT SIDEBAR: (OPTIONAL) PREVIEW + FILE INFO/KEYWORDS + TASKS PANE
        let show_right = self.settings.layout.show_info_pane
            || self.settings.layout.show_tasks_pane
            || (self.settings.layout.show_preview_pane && self.settings.layout.preview_on_right);
        if show_right {
            egui::SidePanel::right("right_dock_panel")
                .frame(dock_frame)
                .resizable(true)
                .default_width(self.settings.layout.info_width)
                .min_width(210.0)
                .max_width(460.0)
                .show(ctx, |ui| {
                    let show_prev_right = self.settings.layout.show_preview_pane
                        && self.settings.layout.preview_on_right;
                    if show_prev_right {
                        let ph = 185.0;
                        ui.allocate_ui(egui::vec2(ui.available_width(), ph), |ui| {
                            preview_pane::show_preview_pane(ui, self, &pal);
                        });
                        ui.add_space(2.0);
                    }

                    if self.settings.layout.show_info_pane && self.settings.layout.show_tasks_pane {
                        let tasks_h = self.settings.layout.tasks_height.clamp(140.0, 300.0);
                        let info_h = (ui.available_height() - tasks_h - 6.0).max(180.0);
                        ui.allocate_ui(egui::vec2(ui.available_width(), info_h), |ui| {
                            info_pane::show_info_pane(ui, self, &pal);
                        });
                        ui.add_space(2.0);
                        tasks_pane::show_tasks_pane(ui, self, &pal);
                    } else if self.settings.layout.show_info_pane {
                        info_pane::show_info_pane(ui, self, &pal);
                    } else if self.settings.layout.show_tasks_pane {
                        tasks_pane::show_tasks_pane(ui, self, &pal);
                    }
                });
        }

        // 6. CENTER WORKSPACE: THUMBNAIL PANE
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg_canvas)
                    .inner_margin(egui::Margin::same(2)),
            )
            .show(ctx, |ui| {
                thumb_pane::show_thumb_pane(ui, self, &pal);
            });

        self.handle_internal_drag_overlay_and_handoff(ctx, &pal);

        // 7. MODALS & KEYBOARD SHORTCUTS
        let modal_open = self.rename_target.is_some()
            || self.rename_ext_confirm.is_some()
            || !self.delete_targets.is_empty()
            || self.folder_create_parent.is_some()
            || self.folder_rename_target.is_some()
            || self.folder_delete_target.is_some()
            || self.similar_dialog_target.is_some();
        if !ctx.wants_keyboard_input() && !modal_open {
            if ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::F3))
            {
                self.start_folder_scan(self.last_folder_context.clone(), true, false);
            } else if ctx
                .input(|i| i.modifiers.shift && i.modifiers.alt && i.key_pressed(egui::Key::F3))
            {
                self.start_folder_scan(self.last_folder_context.clone(), false, false);
            } else if ctx.input(|i| {
                !i.modifiers.ctrl
                    && !i.modifiers.shift
                    && !i.modifiers.alt
                    && i.key_pressed(egui::Key::F3)
            }) {
                self.rebuild_selected_thumbnails();
            } else if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::A)) {
                self.select_all_visible();
            } else if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::Num0)) {
                if let Some(item) = self.selected_item().cloned() {
                    self.open_similar_dialog(item.full_path);
                }
            } else if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter)) {
                self.open_lightbox();
            } else if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::Enter)) {
                self.settings.layout.show_info_pane = true;
            } else if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::Y)) {
                self.show_preferences = true;
                self.prefs_tab = PrefsTab::Ghostscript;
            } else if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
                self.begin_rename_selected();
            } else if ctx.input(|i| {
                i.key_pressed(egui::Key::Delete)
                    || (i.modifiers.ctrl
                        && !i.modifiers.shift
                        && !i.modifiers.alt
                        && i.key_pressed(egui::Key::D))
            }) {
                self.begin_delete_selected();
            }
        }
        dialogs::show_preferences_dialog(ctx, self, &pal);
        dialogs::show_find_similar_dialog(ctx, self);
        dialogs::show_task_properties_dialog(ctx, self);
        dialogs::show_lightbox_modal(ctx, self, &pal);
        dialogs::show_file_action_dialogs(ctx, self);

        // 8. SCREENSHOT AUTOMATION
        self.handle_screenshot(ctx);
    }
}

/// Safely uploads RGBA pixels to an egui GPU texture, automatically clamping dimensions to the
/// GPU's `max_texture_side` so oversized images (e.g. 30,000px CDR strips or panoramas) never panic.
fn load_rgba_texture_clamped(
    ctx: &egui::Context,
    name: String,
    w: u32,
    h: u32,
    raw: &[u8],
    opts: TextureOptions,
) -> Option<TextureHandle> {
    if w == 0 || h == 0 || raw.len() < (w as usize).saturating_mul(h as usize).saturating_mul(4) {
        return None;
    }
    let max_side = (ctx.input(|i| i.max_texture_side) as u32).max(512);
    if w <= max_side && h <= max_side {
        let cimg = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], raw);
        return Some(ctx.load_texture(name, cimg, opts));
    }
    let buf = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(
        w,
        h,
        raw[..(w as usize * h as usize * 4)].to_vec(),
    )?;
    let dyn_img = DynamicImage::ImageRgba8(buf);
    let (dw, dh) = crate::gpu_accel::fit_thumbnail_dims(w, h, max_side);
    let resized = crate::gpu_accel::fast_cpu_area_resize(&dyn_img, dw, dh).to_rgba8();
    let cimg = ColorImage::from_rgba_unmultiplied(
        [resized.width() as usize, resized.height() as usize],
        resized.as_raw(),
    );
    Some(ctx.load_texture(name, cimg, opts))
}

/// Returns the character length of the base filename before its final `.ext` dot,
/// used to auto-select the name minus extension when renaming.
pub fn rename_stem_char_len(file_name: &str) -> usize {
    if let Some(dot_byte) = file_name.rfind('.') {
        if dot_byte > 0 {
            return file_name[..dot_byte].chars().count();
        }
    }
    file_name.chars().count()
}

/// Returns `true` if `new_name` omits a filetype extension or changes the filetype extension
/// compared to `old_path` (case-insensitive).
pub fn needs_extension_change_warning(old_path: &Path, new_name: &str) -> bool {
    let old_ext = old_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let new_ext = Path::new(new_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    new_ext.is_empty() || !old_ext.eq_ignore_ascii_case(new_ext)
}

/// Collects all files in `dir` (and optionally all subfolders) with NO file-count limit.
/// Supported media types receive thumbnails; all other filetypes display their native OS file icon.
fn collect_folder_files(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if recursive {
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let p = entry.path();
            if p.is_file() {
                files.push(p.to_path_buf());
            }
        }
    } else if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_file() {
                files.push(p);
            }
        }
    }
    files.sort_by_cached_key(|p| {
        p.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    });
    files
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dst)?;
    let mut copied = 0usize;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copied += copy_dir_recursive(&entry.path(), &dst_path)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), dst_path)?;
            copied += 1;
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_stem_and_extension_change_warning_behave_accurately() {
        assert_eq!(rename_stem_char_len("photo.jpg"), 5);
        assert_eq!(rename_stem_char_len("archive.tar.gz"), 11);
        assert_eq!(rename_stem_char_len("café.png"), 4);
        assert_eq!(rename_stem_char_len("README"), 6);
        assert_eq!(rename_stem_char_len(".gitignore"), 10);

        // Same extension (case-insensitive) -> no warning
        assert!(!needs_extension_change_warning(Path::new("a/photo.jpg"), "vacation.jpg"));
        assert!(!needs_extension_change_warning(Path::new("a/photo.jpg"), "vacation.JPG"));

        // Changed extension -> warn
        assert!(needs_extension_change_warning(Path::new("a/photo.jpg"), "vacation.png"));

        // Stripped or missing extension -> warn
        assert!(needs_extension_change_warning(Path::new("a/photo.jpg"), "vacation"));
        assert!(needs_extension_change_warning(Path::new("a/photo.jpg"), "vacation."));
        assert!(needs_extension_change_warning(Path::new("a/README"), "README2"));
    }
}
