use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[allow(dead_code)]
pub const DEFAULT_THUMB_WIDTH: u32 = 100; // Thumbs10.exe 0xbc253c
#[allow(dead_code)]
pub const DEFAULT_THUMB_HEIGHT: u32 = 100; // Thumbs10.exe 0xbc2540
#[allow(dead_code)]
pub const DEFAULT_THUMB_JPEG_QUALITY: u8 = 65; // Thumbs10.exe 0xbc2544
pub const DEFAULT_THUMB_QUALITY: u8 = 80; // reference/picture-10.png (Quality 80, WEBP default per user request)
pub const DEFAULT_THUMB_CACHE_DIM: u32 = 120; // reference/picture-10.png (120x120)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThumbFormat {
    Webp,
    Jxl,
    Jpg,
    Png,
}

impl Default for ThumbFormat {
    fn default() -> Self {
        Self::Webp
    }
}

impl ThumbFormat {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Webp => "WEBP (Recommended)",
            Self::Jxl => "JXL (JPEG XL)",
            Self::Jpg => "JPG (JPEG)",
            Self::Png => "PNG (Lossless)",
        }
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            Self::Webp => "WEBP",
            Self::Jxl => "JXL",
            Self::Jpg => "JPG",
            Self::Png => "PNG",
        }
    }

    pub fn ext_tag(&self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Jxl => "jxl",
            Self::Jpg => "jpg",
            Self::Png => "png",
        }
    }

    pub fn supports_quality(&self) -> bool {
        !matches!(self, Self::Png)
    }

    pub fn all() -> &'static [Self] {
        &[Self::Webp, Self::Jxl, Self::Jpg, Self::Png]
    }
}

fn default_thumb_quality() -> u8 {
    DEFAULT_THUMB_QUALITY
}

fn default_thumb_cache_dim() -> u32 {
    DEFAULT_THUMB_CACHE_DIM
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchScope {
    EntireDatabase,
    CurrentTree,
    CurrentFolder,
}

impl Default for SearchScope {
    fn default() -> Self {
        Self::EntireDatabase
    }
}

impl SearchScope {
    pub fn label(&self) -> &'static str {
        match self {
            Self::EntireDatabase => "Entire Database",
            Self::CurrentTree => "Current Tree",
            Self::CurrentFolder => "Current Folder",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::EntireDatabase, Self::CurrentTree, Self::CurrentFolder]
    }
}
#[allow(dead_code)]
pub const DEFAULT_TREE_PANE_WIDTH: f32 = 220.0; // Thumbs10.exe 0x648c70 (200px min/default)
#[allow(dead_code)]
pub const DEFAULT_PREVIEW_PANE_HEIGHT: f32 = 210.0; // Thumbs10.exe 0x648c70 (200px)
#[allow(dead_code)]
pub const DEFAULT_INFO_PANE_WIDTH: f32 = 275.0; // Thumbs10.exe 0x648c70 (200px+)
#[allow(dead_code)]
pub const DEFAULT_TASKS_PANE_HEIGHT: f32 = 155.0; // Thumbs10.exe 0x648c70 (200px)

pub const GS_MIN_DPI: u32 = 50; // plug_gs.tpp FUN_100012c0 (0x32)
pub const GS_MAX_DPI: u32 = 1440; // plug_gs.tpp FUN_100012c0 (0x5a0)
pub const GS_DPI_PRESETS: &[u32] = &[50, 72, 100, 150, 200, 300, 600]; // plug_gs.tpp FUN_100015c0 (0x100226b0)
pub const GS_MIN_REVISION: u32 = 800; // plug_gs.tpp FUN_10001830 (799 < revision)
pub const GS_HEADER_PEEK_BYTES: usize = 200; // plug_gs.tpp FUN_10001c80 (_fread 200)
pub const GS_DPI_TO_PPM_FACTOR: f64 = 39.37; // plug_gs.tpp FUN_10001830 (_DAT_10022770)
pub const GS_DEFAULT_THUMB_DPI: u32 = 100; // plug_gs.tpp DAT_10029000 (ThumbDPI)
pub const GS_DEFAULT_PREVIEW_DPI: u32 = 150; // plug_gs.tpp DAT_10029004 (ViewDPI)
pub const GS_DEFAULT_FULL_DPI: u32 = 300; // plug_gs.tpp DAT_10029008 (PrintDPI)
pub const GS_DEFAULT_AA_BITS: u8 = 4; // plug_gs.tpp DAT_1002900c (AntiAlias)
pub const GS_DEFAULT_COLOR_DEPTH: u8 = 24; // plug_gs.tpp DAT_10029010 (Format)
pub const MAX_MD5_FILE_KB: u64 = 16384; // Thumbs10.exe table 0xa6d3fc (MaxMD5Kb = 16 MB)

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppTheme {
    ClassicLight,
    StudioNeutral,
    Darkroom,
    HighContrast,
}

impl AppTheme {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ClassicLight => "Classic Light",
            Self::StudioNeutral => "Studio Neutral",
            Self::Darkroom => "Darkroom",
            Self::HighContrast => "High Contrast",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::ClassicLight,
            Self::StudioNeutral,
            Self::Darkroom,
            Self::HighContrast,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutPreset {
    ClassicFivePane,
    ExplorerCompact,
    LightTable,
    Filmstrip,
    MetadataFocus,
}

impl LayoutPreset {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ClassicFivePane => "Classic 5-Pane",
            Self::ExplorerCompact => "Explorer Compact",
            Self::LightTable => "Light Table",
            Self::Filmstrip => "Filmstrip Studio",
            Self::MetadataFocus => "Metadata & Keywords",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::ClassicFivePane,
            Self::ExplorerCompact,
            Self::LightTable,
            Self::Filmstrip,
            Self::MetadataFocus,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    Thumbnails,
    Details,
    Filmstrip,
}

impl ViewMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Thumbnails => "Thumbnails",
            Self::Details => "Details",
            Self::Filmstrip => "Filmstrip",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortField {
    Name,
    FileType,
    Size,
    Modified,
    Dimensions,
    Rating,
    KeywordCount,
    Similarity,
}

impl SortField {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::FileType => "Format",
            Self::Size => "File Size",
            Self::Modified => "Modified",
            Self::Dimensions => "Dimensions",
            Self::Rating => "Rating",
            Self::KeywordCount => "Keywords",
            Self::Similarity => "Similarity",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Name,
            Self::FileType,
            Self::Size,
            Self::Modified,
            Self::Dimensions,
            Self::Rating,
            Self::KeywordCount,
            Self::Similarity,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionOptions {
    pub show_name: bool,
    pub show_size: bool,
    pub show_dimensions: bool,
    pub show_format_badge: bool,
    pub show_rating: bool,
    pub show_keywords: bool,
}

impl Default for CaptionOptions {
    fn default() -> Self {
        Self {
            show_name: true,
            show_size: true,
            show_dimensions: true,
            show_format_badge: true,
            show_rating: true,
            show_keywords: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhostscriptConfig {
    pub gs_path: String,
    pub cdr_converter_path: String,
    pub thumbnail_dpi: u32,
    pub preview_dpi: u32,
    pub full_view_dpi: u32,
    pub antialias_bits: u8,
    pub color_depth: u8,
    pub eps_crop: bool,
    pub safer_mode: bool,
}

impl Default for GhostscriptConfig {
    fn default() -> Self {
        Self {
            gs_path: crate::ghostscript::detect_ghostscript().unwrap_or_default(),
            cdr_converter_path: crate::ghostscript::detect_cdr_converter().unwrap_or_default(),
            thumbnail_dpi: GS_DEFAULT_THUMB_DPI,
            preview_dpi: GS_DEFAULT_PREVIEW_DPI,
            full_view_dpi: GS_DEFAULT_FULL_DPI,
            antialias_bits: GS_DEFAULT_AA_BITS,
            color_depth: GS_DEFAULT_COLOR_DEPTH,
            eps_crop: true,
            safer_mode: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiLayoutConfig {
    pub show_tree_pane: bool,
    pub show_preview_pane: bool,
    pub show_info_pane: bool,
    pub show_tasks_pane: bool,
    pub show_toolbar: bool,
    pub show_status_bar: bool,
    pub preview_on_right: bool,
    pub tree_width: f32,
    pub preview_height: f32,
    pub info_width: f32,
    pub tasks_height: f32,
    pub card_spacing: f32,
    pub card_rounding: f32,
    pub checkerboard_alpha: bool,
}

impl Default for UiLayoutConfig {
    fn default() -> Self {
        Self {
            show_tree_pane: true,
            show_preview_pane: true,
            show_info_pane: true,
            show_tasks_pane: true,
            show_toolbar: true,
            show_status_bar: true,
            preview_on_right: false,
            tree_width: 250.0,
            preview_height: 225.0,
            info_width: 265.0,
            tasks_height: 195.0,
            card_spacing: 8.0,
            card_rounding: 4.0,
            checkerboard_alpha: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: AppTheme,
    pub layout: UiLayoutConfig,
    pub view_mode: ViewMode,
    pub thumb_size: f32,
    pub sort_field: SortField,
    pub sort_descending: bool,
    pub captions: CaptionOptions,
    pub ghostscript: GhostscriptConfig,
    pub include_subfolders: bool,
    pub auto_extract_cdr_keywords: bool,
    pub last_folder: Option<String>,
    #[serde(default)]
    pub thumb_format: ThumbFormat,
    #[serde(default = "default_thumb_quality")]
    pub thumb_quality: u8,
    #[serde(default = "default_thumb_cache_dim")]
    pub thumb_cache_dim: u32,
    #[serde(default)]
    pub favorite_folders: Vec<String>,
    #[serde(default)]
    pub custom_db_path: Option<String>,
    #[serde(default)]
    pub search_scope: SearchScope,
    #[serde(default)]
    pub auto_clear_completed_tasks: bool,
    #[serde(default = "default_true")]
    pub write_keywords_to_files: bool,
    #[serde(default = "default_true")]
    pub use_gpu_acceleration: bool,
    #[serde(default)]
    pub cpu_worker_threads: usize,
    #[serde(default = "default_true")]
    pub auto_scan: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: AppTheme::ClassicLight,
            layout: UiLayoutConfig::default(),
            view_mode: ViewMode::Thumbnails,
            thumb_size: 120.0,
            sort_field: SortField::Name,
            sort_descending: false,
            captions: CaptionOptions::default(),
            ghostscript: GhostscriptConfig::default(),
            include_subfolders: false,
            auto_extract_cdr_keywords: true,
            last_folder: None,
            thumb_format: ThumbFormat::Webp,
            thumb_quality: DEFAULT_THUMB_QUALITY,
            thumb_cache_dim: DEFAULT_THUMB_CACHE_DIM,
            favorite_folders: Vec::new(),
            custom_db_path: None,
            search_scope: SearchScope::EntireDatabase,
            auto_clear_completed_tasks: false,
            write_keywords_to_files: true,
            use_gpu_acceleration: true,
            cpu_worker_threads: 0,
            auto_scan: true,
        }
    }
}

impl AppSettings {
    pub fn apply_preset(&mut self, preset: LayoutPreset) {
        match preset {
            LayoutPreset::ClassicFivePane => {
                self.layout.show_tree_pane = true;
                self.layout.show_preview_pane = true;
                self.layout.show_info_pane = true;
                self.layout.show_tasks_pane = true;
                self.layout.preview_on_right = false;
                self.view_mode = ViewMode::Thumbnails;
            }
            LayoutPreset::ExplorerCompact => {
                self.layout.show_tree_pane = true;
                self.layout.show_preview_pane = false;
                self.layout.show_info_pane = false;
                self.layout.show_tasks_pane = false;
                self.view_mode = ViewMode::Thumbnails;
            }
            LayoutPreset::LightTable => {
                self.layout.show_tree_pane = false;
                self.layout.show_preview_pane = false;
                self.layout.show_info_pane = false;
                self.layout.show_tasks_pane = false;
                self.view_mode = ViewMode::Thumbnails;
                self.thumb_size = 180.0;
            }
            LayoutPreset::Filmstrip => {
                self.layout.show_tree_pane = true;
                self.layout.show_preview_pane = false;
                self.layout.show_info_pane = true;
                self.layout.show_tasks_pane = false;
                self.view_mode = ViewMode::Filmstrip;
            }
            LayoutPreset::MetadataFocus => {
                self.layout.show_tree_pane = true;
                self.layout.show_preview_pane = true;
                self.layout.show_info_pane = true;
                self.layout.show_tasks_pane = true;
                self.layout.preview_on_right = true;
                self.layout.info_width = 330.0;
                self.view_mode = ViewMode::Details;
            }
        }
    }

    pub fn settings_path() -> PathBuf {
        let mut dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        dir.push("thumbcraft_settings.json");
        dir
    }

    pub fn load() -> Self {
        let path = Self::settings_path();
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(mut settings) = serde_json::from_str::<Self>(&raw) {
                if settings.ghostscript.gs_path.is_empty() {
                    settings.ghostscript.gs_path =
                        crate::ghostscript::detect_ghostscript().unwrap_or_default();
                }
                settings.ghostscript.thumbnail_dpi =
                    crate::ghostscript::clamp_dpi(settings.ghostscript.thumbnail_dpi);
                settings.ghostscript.preview_dpi =
                    crate::ghostscript::clamp_dpi(settings.ghostscript.preview_dpi);
                settings.ghostscript.full_view_dpi =
                    crate::ghostscript::clamp_dpi(settings.ghostscript.full_view_dpi);
                return settings;
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = Self::settings_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}
