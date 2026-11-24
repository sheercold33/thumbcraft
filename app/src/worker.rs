use crate::catalog::CatalogDb;
use crate::config::{GhostscriptConfig, ThumbFormat, MAX_MD5_FILE_KB};
use crate::decoder::{self, DecodedMedia};
use crate::gpu_accel;
use crate::similarity::{self, VisualSignature};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const FLUSH_BATCH_THRESHOLD: usize = 400;
const FLUSH_INTERVAL_SECS: u64 = 4;

#[derive(Debug, Clone)]
pub struct ThumbEncodeConfig {
    pub format: ThumbFormat,
    pub quality: u8,
    pub max_dim: u32,
    pub use_gpu: bool,
    pub cpu_threads: usize,
}

#[derive(Clone)]
pub struct WorkerRequest {
    pub files: Vec<PathBuf>,
    pub gs_config: GhostscriptConfig,
    pub thumb_cfg: ThumbEncodeConfig,
    pub db: Arc<Mutex<CatalogDb>>,
    pub force_redecode: bool,
    pub generation: u64,
    /// When true (large folders), sends compressed thumbnail blobs instead of raw RGBA to keep RAM tiny.
    pub send_blob_only: bool,
    /// When true (Auto-Scan OFF), loads already-cached DB thumbnails/metadata without decoding uncached files.
    pub cache_only: bool,
}

#[derive(Debug, Clone)]
pub enum WorkerEvent {
    ItemReady {
        generation: u64,
        path: PathBuf,
        filetype: String,
        width: u32,
        height: u32,
        color_depth: u16,
        page_count: u32,
        file_length: u64,
        file_time_iso: String,
        rating: u8,
        annotation: String,
        keywords: Vec<String>,
        metadata: BTreeMap<String, String>,
        rgba: Option<(u32, u32, Vec<u8>)>,
        thumb_blob: Option<(Vec<u8>, String)>,
        md5: String,
        source_note: String,
    },
    Progress {
        generation: u64,
        done: usize,
        total: usize,
        current_file: String,
    },
    Log(String),
    BatchDone {
        generation: u64,
        total: usize,
    },
}

pub struct BackgroundWorker {
    tx_req: Sender<WorkerRequest>,
    rx_evt: Receiver<WorkerEvent>,
}

impl BackgroundWorker {
    pub fn spawn() -> Self {
        let (tx_req, rx_req) = mpsc::channel::<WorkerRequest>();
        let (tx_evt, rx_evt) = mpsc::channel::<WorkerEvent>();

        thread::spawn(move || {
            let mut pending: Option<WorkerRequest> = None;
            loop {
                let mut req = match pending.take() {
                    Some(r) => r,
                    None => match rx_req.recv() {
                        Ok(r) => r,
                        Err(_) => break,
                    },
                };
                while let Ok(newer) = rx_req.try_recv() {
                    req = newer;
                }
                pending = process_batch(&req, &rx_req, &tx_evt);
            }
        });

        Self { tx_req, rx_evt }
    }

    pub fn submit(&self, req: WorkerRequest) {
        let _ = self.tx_req.send(req);
    }

    pub fn drain_events(&self) -> Vec<WorkerEvent> {
        let mut out = Vec::new();
        while let Ok(evt) = self.rx_evt.try_recv() {
            out.push(evt);
        }
        out
    }
}

struct DecodedThumbPayload {
    path: PathBuf,
    file_length: u64,
    file_time_iso: String,
    decoded: DecodedMedia,
    md5_hex: String,
    rgba: Option<(u32, u32, Vec<u8>)>,
    encoded_blob: Option<(Vec<u8>, String)>,
    visual_sig: Option<VisualSignature>,
}

fn decode_and_encode_one(
    path: &Path,
    file_length: u64,
    file_time_iso: String,
    gs_config: &GhostscriptConfig,
    thumb_cfg: &ThumbEncodeConfig,
    want_rgba: bool,
) -> DecodedThumbPayload {
    let max_md5_bytes = MAX_MD5_FILE_KB * 1024;
    let decoded: DecodedMedia = decoder::decode_thumbnail(path, gs_config);
    let md5_hex = decoded.md5.clone().unwrap_or_else(|| {
        similarity::md5_file_capped(path, max_md5_bytes)
            .ok()
            .flatten()
            .unwrap_or_default()
    });

    let (rgba, encoded_blob, visual_sig) = if let Some(img) = decoded.image.as_ref() {
        let max_cache = thumb_cfg.max_dim.clamp(64, 320);
        let (thumb, _used_gpu) =
            gpu_accel::resize_thumbnail_hybrid(img.clone(), max_cache, thumb_cfg.use_gpu);
        let sig = VisualSignature::from_image(&thumb);
        let (blob, fmt_tag) = decoder::encode_thumbnail(&thumb, thumb_cfg.format, thumb_cfg.quality);
        let rgba_opt = if want_rgba {
            let r = thumb.to_rgba8();
            Some((r.width(), r.height(), r.into_raw()))
        } else {
            None
        };
        (rgba_opt, Some((blob, fmt_tag.to_string())), Some(sig))
    } else {
        (None, None, None)
    };

    DecodedThumbPayload {
        path: path.to_path_buf(),
        file_length,
        file_time_iso,
        decoded,
        md5_hex,
        rgba,
        encoded_blob,
        visual_sig,
    }
}

fn run_parallel<T: Sync, R: Send, F: Fn(&T) -> R + Sync + Send>(
    items: &[T],
    cpu_threads: usize,
    f: F,
) -> Vec<R> {
    if items.len() <= 1 || cpu_threads == 1 {
        return items.iter().map(&f).collect();
    }
    if cpu_threads > 1 && cpu_threads < rayon::current_num_threads() {
        if let Ok(pool) = rayon::ThreadPoolBuilder::new()
            .num_threads(cpu_threads)
            .build()
        {
            return pool.install(|| items.par_iter().map(&f).collect());
        }
    }
    items.par_iter().map(&f).collect()
}

fn process_batch(
    req: &WorkerRequest,
    rx_req: &Receiver<WorkerRequest>,
    tx_evt: &Sender<WorkerEvent>,
) -> Option<WorkerRequest> {
    let total = req.files.len();
    let max_md5_bytes = MAX_MD5_FILE_KB * 1024;
    let accel_tag = if req.thumb_cfg.use_gpu && gpu_accel::gpu_adapter_info().is_some() {
        "GPU+CPU"
    } else {
        "Multi-CPU"
    };
    let _ = tx_evt.send(WorkerEvent::Log(format!(
        "Indexing {} file(s) ({} Q{}, {}px | {} | GS DPI: {})...",
        total,
        req.thumb_cfg.format.short_name(),
        req.thumb_cfg.quality,
        req.thumb_cfg.max_dim,
        accel_tag,
        req.gs_config.thumbnail_dpi
    )));

    let mut dirty_since_flush = 0usize;
    let mut last_flush = Instant::now();
    let mut next_req: Option<WorkerRequest> = None;
    let mut processed = 0usize;

    // Process in responsive parallel chunks so folder switches interrupt immediately
    // while uncached files decode/resize/encode across all CPU cores + GPU.
    let chunk_size = if total <= 32 { 8 } else { 20 };

    for chunk in req.files.chunks(chunk_size) {
        if let Ok(mut newer) = rx_req.try_recv() {
            while let Ok(even_newer) = rx_req.try_recv() {
                newer = even_newer;
            }
            next_req = Some(newer);
            break;
        }

        if let Some(first_path) = chunk.first() {
            let fname = first_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let _ = tx_evt.send(WorkerEvent::Progress {
                generation: req.generation,
                done: processed,
                total,
                current_file: fname,
            });
        }

        let stats: Vec<(PathBuf, u64, String)> = chunk
            .iter()
            .map(|p| {
                let (len, mtime) = file_stat(p);
                (p.clone(), len, mtime)
            })
            .collect();

        let mut to_decode: Vec<(PathBuf, u64, String)> = Vec::new();
        let mut cache_hits = Vec::new();

        if !req.force_redecode {
            if let Ok(cat) = req.db.lock() {
                for (path, file_length, file_time_iso) in &stats {
                    let hit = cat.get_by_path(path).ok().flatten().filter(|cached| {
                        cached.file_length == *file_length
                            && cached.file_time_iso == *file_time_iso
                            && cached
                                .thumb_blob
                                .as_ref()
                                .map(|b| !b.is_empty())
                                .unwrap_or(false)
                    });
                    if let Some(cached) = hit {
                        cache_hits.push((path.clone(), *file_length, file_time_iso.clone(), cached));
                    } else {
                        to_decode.push((path.clone(), *file_length, file_time_iso.clone()));
                    }
                }
            } else {
                to_decode = stats;
            }
        } else {
            to_decode = stats;
        }

        // Decode cached thumbnail blobs to RGBA in parallel if needed
        let cached_events: Vec<Option<WorkerEvent>> =
            run_parallel(&cache_hits, req.thumb_cfg.cpu_threads, |(path, file_length, file_time_iso, cached)| {
                let blob = cached.thumb_blob.as_ref()?.clone();
                let (rgba, thumb_blob) = if req.send_blob_only {
                    (None, Some((blob, cached.thumb_fmt.clone())))
                } else {
                    let decoded_rgba = decoder::decode_thumbnail_blob(&blob, &cached.thumb_fmt).map(|img| {
                        let r = img.to_rgba8();
                        (r.width(), r.height(), r.into_raw())
                    });
                    (decoded_rgba, Some((blob, cached.thumb_fmt.clone())))
                };
                if rgba.is_none() && !req.send_blob_only {
                    return None;
                }
                let md5_val = if cached.md5.is_empty() {
                    similarity::md5_file_capped(path, max_md5_bytes)
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                } else {
                    cached.md5.clone()
                };
                Some(WorkerEvent::ItemReady {
                    generation: req.generation,
                    path: path.clone(),
                    filetype: cached.filetype.clone(),
                    width: cached.width,
                    height: cached.height,
                    color_depth: cached.color_depth,
                    page_count: cached.page_count,
                    file_length: *file_length,
                    file_time_iso: file_time_iso.clone(),
                    rating: cached.rating,
                    annotation: cached.annotation.clone(),
                    keywords: cached.keywords.clone(),
                    metadata: cached.metadata.clone(),
                    rgba,
                    thumb_blob,
                    md5: md5_val,
                    source_note: format!("DB ({})", cached.thumb_fmt.to_uppercase()),
                })
            });

        for (i, evt_opt) in cached_events.into_iter().enumerate() {
            if let Some(evt) = evt_opt {
                let _ = tx_evt.send(evt);
                processed += 1;
            } else {
                let (p, len, mtime, _) = &cache_hits[i];
                to_decode.push((p.clone(), *len, mtime.clone()));
            }
        }

        if !to_decode.is_empty() && req.cache_only {
            for (path, file_length, file_time_iso) in to_decode {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_uppercase())
                    .unwrap_or_else(|| "FILE".to_string());
                let _ = tx_evt.send(WorkerEvent::ItemReady {
                    generation: req.generation,
                    path,
                    filetype: ext,
                    width: 0,
                    height: 0,
                    color_depth: 0,
                    page_count: 1,
                    file_length,
                    file_time_iso,
                    rating: 0,
                    annotation: String::new(),
                    keywords: Vec::new(),
                    metadata: BTreeMap::new(),
                    rgba: None,
                    thumb_blob: None,
                    md5: String::new(),
                    source_note: "Not scanned".to_string(),
                });
                processed += 1;
            }
        } else if !to_decode.is_empty() {
            let decoded_results: Vec<DecodedThumbPayload> =
                run_parallel(&to_decode, req.thumb_cfg.cpu_threads, |(path, len, mtime)| {
                    decode_and_encode_one(
                        path,
                        *len,
                        mtime.clone(),
                        &req.gs_config,
                        &req.thumb_cfg,
                        !req.send_blob_only,
                    )
                });

            let mut db_meta_out: Vec<(u8, String, Vec<String>)> =
                Vec::with_capacity(decoded_results.len());

            if let Ok(mut cat) = req.db.lock() {
                for item in &decoded_results {
                    let blob_ref = item
                        .encoded_blob
                        .as_ref()
                        .map(|(b, f)| (b.as_slice(), f.as_str()));
                    let _ = cat.upsert_thumbnail_batch(
                        &item.path,
                        &item.decoded.filetype,
                        &item.file_time_iso,
                        item.file_length,
                        item.decoded.width,
                        item.decoded.height,
                        item.decoded.color_depth,
                        item.decoded.page_count,
                        &item.decoded.keywords,
                        &item.decoded.metadata,
                        blob_ref,
                        &item.md5_hex,
                        item.visual_sig.clone(),
                    );
                    if let Ok(Some(rec)) = cat.get_by_path(&item.path) {
                        db_meta_out.push((rec.rating, rec.annotation, rec.keywords));
                    } else {
                        db_meta_out.push((0, String::new(), item.decoded.keywords.clone()));
                    }
                    dirty_since_flush += 1;
                }
                if dirty_since_flush >= FLUSH_BATCH_THRESHOLD
                    || (dirty_since_flush > 0
                        && last_flush.elapsed() >= Duration::from_secs(FLUSH_INTERVAL_SECS))
                {
                    let _ = cat.flush();
                    dirty_since_flush = 0;
                    last_flush = Instant::now();
                }
            } else {
                for item in &decoded_results {
                    db_meta_out.push((0, String::new(), item.decoded.keywords.clone()));
                }
            }

            for (item, (rating, annotation, merged_keywords)) in
                decoded_results.into_iter().zip(db_meta_out.into_iter())
            {
                if total <= 100 && !item.decoded.log_line.is_empty() {
                    let _ = tx_evt.send(WorkerEvent::Log(item.decoded.log_line));
                }
                let _ = tx_evt.send(WorkerEvent::ItemReady {
                    generation: req.generation,
                    path: item.path,
                    filetype: item.decoded.filetype,
                    width: item.decoded.width,
                    height: item.decoded.height,
                    color_depth: item.decoded.color_depth,
                    page_count: item.decoded.page_count,
                    file_length: item.file_length,
                    file_time_iso: item.file_time_iso,
                    rating,
                    annotation,
                    keywords: merged_keywords,
                    metadata: item.decoded.metadata,
                    rgba: item.rgba,
                    thumb_blob: item.encoded_blob,
                    md5: item.md5_hex,
                    source_note: item.decoded.source_note,
                });
                processed += 1;
            }
        }
    }

    if dirty_since_flush > 0 {
        if let Ok(mut cat) = req.db.lock() {
            let _ = cat.flush();
        }
    }

    if next_req.is_none() {
        let _ = tx_evt.send(WorkerEvent::BatchDone {
            generation: req.generation,
            total,
        });
    }
    next_req
}

// ---------------------------------------------------------------------------
// Managed Background Task Queue (Scan Folder / Scan Tree / Remake / F3 Rebuild Selected)
// Supports per-task Pause, Resume, Quit, Thread ID, Elapsed & Active time, and Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Queued,
    Running,
    Paused,
    Completed,
    Cancelled,
}

impl TaskState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Completed => "Completed",
            Self::Cancelled => "Cancelled",
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }
}

#[derive(Default)]
pub struct TaskControl {
    pub paused: AtomicBool,
    pub cancelled: AtomicBool,
}

#[derive(Clone)]
pub struct FolderScanJob {
    pub task_id: u64,
    pub display_text: String,
    pub root_dir: PathBuf,
    /// When `Some`, processes ONLY these explicit files (used by F3 Rebuild Selected!).
    pub explicit_files: Option<Vec<PathBuf>>,
    pub recursive: bool,
    pub force_redecode: bool,
    pub gs_config: GhostscriptConfig,
    pub thumb_cfg: ThumbEncodeConfig,
    pub db: Arc<Mutex<CatalogDb>>,
    pub control: Arc<TaskControl>,
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Started {
        task_id: u64,
        started_at: String,
        thread_id: u64,
        total: usize,
        seen: usize,
    },
    Progress {
        task_id: u64,
        done: usize,
        total: usize,
        seen: usize,
        indexed: usize,
        skipped: usize,
        current_file: String,
        elapsed_ms: u64,
        active_ms: u64,
        state: TaskState,
    },
    ItemRefreshed {
        path: PathBuf,
        filetype: String,
        width: u32,
        height: u32,
        color_depth: u16,
        page_count: u32,
        file_length: u64,
        file_time_iso: String,
        keywords: Vec<String>,
        metadata: BTreeMap<String, String>,
        thumb_blob: Option<(Vec<u8>, String)>,
        md5: String,
        source_note: String,
    },
    TaskMessage {
        task_id: u64,
        message: String,
        is_error: bool,
    },
    Log(String),
    Finished {
        task_id: u64,
        completed_at: String,
        elapsed_ms: u64,
        active_ms: u64,
        done: usize,
        total: usize,
        seen: usize,
        indexed: usize,
        skipped: usize,
        cancelled: bool,
        scanned_dirs: Vec<PathBuf>,
    },
}

pub struct FolderScanner {
    tx_job: Sender<FolderScanJob>,
    rx_evt: Receiver<ScanEvent>,
}

impl FolderScanner {
    pub fn spawn() -> Self {
        let (tx_job, rx_job) = mpsc::channel::<FolderScanJob>();
        let (tx_evt, rx_evt) = mpsc::channel::<ScanEvent>();
        let shared_rx = Arc::new(Mutex::new(rx_job));

        // Spawn a pool of concurrent scan workers so multiple Scan Folder / Scan Tree / Remake
        // tasks can run simultaneously in parallel, each with independent Pause/Resume/Cancel.
        let worker_count = thread::available_parallelism()
            .map(|n| n.get().clamp(3, 6))
            .unwrap_or(4);

        for _ in 0..worker_count {
            let rx_clone = shared_rx.clone();
            let tx_clone = tx_evt.clone();
            thread::spawn(move || {
                let tid = current_thread_numeric_id();
                loop {
                    let job = {
                        let Ok(guard) = rx_clone.lock() else {
                            break;
                        };
                        match guard.recv() {
                            Ok(j) => j,
                            Err(_) => break,
                        }
                    };
                    run_folder_scan(&job, tid, &tx_clone);
                }
            });
        }

        Self { tx_job, rx_evt }
    }

    pub fn enqueue(&self, job: FolderScanJob) {
        let _ = self.tx_job.send(job);
    }

    pub fn drain_events(&self) -> Vec<ScanEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx_evt.try_recv() {
            out.push(e);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Async Preview Decoder Worker (keeps clicking/arrow-keying 100% lag-free)
// ---------------------------------------------------------------------------

pub struct PreviewRequest {
    pub generation: u64,
    pub path: PathBuf,
    pub gs_config: GhostscriptConfig,
    pub page: usize,
    pub full_res: bool,
    pub max_preview_dim: u32,
    pub ctx: Option<egui::Context>,
}

pub struct PreviewEvent {
    pub generation: u64,
    pub path: PathBuf,
    pub image: Option<image::DynamicImage>,
    pub tech_meta: BTreeMap<String, String>,
}

pub struct PreviewWorker {
    tx_req: Sender<PreviewRequest>,
    rx_evt: Receiver<PreviewEvent>,
}

impl PreviewWorker {
    pub fn spawn() -> Self {
        let (tx_req, rx_req) = mpsc::channel::<PreviewRequest>();
        let (tx_evt, rx_evt) = mpsc::channel::<PreviewEvent>();

        thread::spawn(move || {
            while let Ok(mut req) = rx_req.recv() {
                // Coalesce rapid clicks/arrow-key presses so obsolete decodes are skipped
                while let Ok(newer) = rx_req.try_recv() {
                    req = newer;
                }
                let decoded = decoder::decode_preview_page(
                    &req.path,
                    &req.gs_config,
                    req.page,
                    req.full_res,
                );
                let mut tech_meta = BTreeMap::new();
                for k in [
                    crate::image_meta::KEY_SUBSAMPLING,
                    crate::image_meta::KEY_ESTIMATED_QUALITY,
                    crate::image_meta::KEY_BITS_PER_CHANNEL,
                    crate::image_meta::KEY_DPI,
                ] {
                    if let Some(v) = decoded.metadata.get(k) {
                        tech_meta.insert(k.to_string(), v.clone());
                    }
                }
                if tech_meta.len() < 4 {
                    let probed = crate::image_meta::inspect_file_technical_metadata(
                        &req.path,
                        Some(&req.gs_config),
                    );
                    let mut probed_map = BTreeMap::new();
                    probed.insert_into(&mut probed_map);
                    for (k, v) in probed_map {
                        tech_meta.entry(k).or_insert(v);
                    }
                }
                let clamped_image = decoded.image.map(|img| {
                    let limit = req.max_preview_dim.max(512);
                    if img.width() > limit || img.height() > limit {
                        let (dw, dh) =
                            gpu_accel::fit_thumbnail_dims(img.width(), img.height(), limit);
                        gpu_accel::fast_cpu_area_resize(&img, dw, dh)
                    } else {
                        img
                    }
                });
                let _ = tx_evt.send(PreviewEvent {
                    generation: req.generation,
                    path: req.path,
                    image: clamped_image,
                    tech_meta,
                });
                if let Some(ref ctx) = req.ctx {
                    ctx.request_repaint();
                }
            }
        });

        Self { tx_req, rx_evt }
    }

    pub fn submit(&self, req: PreviewRequest) {
        let _ = self.tx_req.send(req);
    }

    pub fn drain_events(&self) -> Vec<PreviewEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx_evt.try_recv() {
            out.push(e);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Async Thumbnail Blob -> RGBA Pool (offloads WEBP/JXL decode during scroll)
// ---------------------------------------------------------------------------

pub struct ThumbDecodeRequest {
    pub generation: u64,
    pub index: usize,
    pub blob: Arc<Vec<u8>>,
    pub fmt: String,
    pub ctx: Option<egui::Context>,
}

pub struct ThumbDecodeEvent {
    pub generation: u64,
    pub index: usize,
    pub rgba: Option<(u32, u32, Vec<u8>)>,
}

pub struct ThumbDecodePool {
    tx_req: Sender<ThumbDecodeRequest>,
    rx_evt: Receiver<ThumbDecodeEvent>,
}

impl ThumbDecodePool {
    pub fn spawn() -> Self {
        let (tx_req, rx_req) = mpsc::channel::<ThumbDecodeRequest>();
        let (tx_evt, rx_evt) = mpsc::channel::<ThumbDecodeEvent>();

        thread::spawn(move || {
            while let Ok(first) = rx_req.recv() {
                let mut batch = vec![first];
                while batch.len() < 32 {
                    match rx_req.try_recv() {
                        Ok(more) => batch.push(more),
                        Err(_) => break,
                    }
                }
                // Drop stale requests if a newer folder generation arrived in the same burst
                let latest_gen = batch.iter().map(|r| r.generation).max().unwrap_or(0);
                let wake_ctx = batch
                    .iter()
                    .rev()
                    .find_map(|r| r.ctx.clone());
                let active: Vec<ThumbDecodeRequest> = batch
                    .into_iter()
                    .filter(|r| r.generation == latest_gen)
                    .collect();

                let results: Vec<ThumbDecodeEvent> = active
                    .par_iter()
                    .map(|req| {
                        let rgba = decoder::decode_thumbnail_blob(&req.blob, &req.fmt).map(|img| {
                            let r = img.to_rgba8();
                            (r.width(), r.height(), r.into_raw())
                        });
                        ThumbDecodeEvent {
                            generation: req.generation,
                            index: req.index,
                            rgba,
                        }
                    })
                    .collect();

                for evt in results {
                    let _ = tx_evt.send(evt);
                }
                if let Some(ctx) = wake_ctx {
                    ctx.request_repaint();
                }
            }
        });

        Self { tx_req, rx_evt }
    }

    pub fn submit(&self, req: ThumbDecodeRequest) {
        let _ = self.tx_req.send(req);
    }

    pub fn drain_events(&self) -> Vec<ThumbDecodeEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx_evt.try_recv() {
            out.push(e);
        }
        out
    }
}

fn current_thread_numeric_id() -> u64 {
    let s = format!("{:?}", thread::current().id());
    s.chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<u64>()
        .unwrap_or(1)
}

fn run_folder_scan(job: &FolderScanJob, thread_id: u64, tx_evt: &Sender<ScanEvent>) {
    let wall_start = Instant::now();
    let started_at = chrono::Local::now().format("%m/%d/%Y  %I:%M:%S %p").to_string();

    if job.control.cancelled.load(Ordering::Relaxed) {
        let _ = tx_evt.send(ScanEvent::Finished {
            task_id: job.task_id,
            completed_at: started_at,
            elapsed_ms: 0,
            active_ms: 0,
            done: 0,
            total: 0,
            seen: 0,
            indexed: 0,
            skipped: 0,
            cancelled: true,
            scanned_dirs: Vec::new(),
        });
        return;
    }

    // If paused while queued, wait here before walking disk
    while job.control.paused.load(Ordering::SeqCst) {
        if job.control.cancelled.load(Ordering::SeqCst) {
            break;
        }
        thread::sleep(Duration::from_millis(60));
    }
    if job.control.cancelled.load(Ordering::SeqCst) {
        let _ = tx_evt.send(ScanEvent::Finished {
            task_id: job.task_id,
            completed_at: chrono::Local::now()
                .format("%m/%d/%Y  %I:%M:%S %p")
                .to_string(),
            elapsed_ms: wall_start.elapsed().as_millis() as u64,
            active_ms: 0,
            done: 0,
            total: 0,
            seen: 0,
            indexed: 0,
            skipped: 0,
            cancelled: true,
            scanned_dirs: Vec::new(),
        });
        return;
    }

    let _ = tx_evt.send(ScanEvent::Log(format!(
        "{} — starting...",
        job.display_text
    )));

    let mut seen = 0usize;
    let mut visited_dirs: Vec<PathBuf> = Vec::new();
    let files: Vec<PathBuf> = if let Some(ref explicit) = job.explicit_files {
        seen = explicit.len();
        explicit.clone()
    } else {
        let mut discovered = Vec::new();
        visited_dirs.push(job.root_dir.clone());
        if job.recursive {
            for entry in walkdir::WalkDir::new(&job.root_dir).into_iter().flatten() {
                while job.control.paused.load(Ordering::Relaxed) {
                    if job.control.cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(60));
                }
                if job.control.cancelled.load(Ordering::Relaxed) {
                    break;
                }
                let p = entry.path();
                if p.is_dir() {
                    if p != job.root_dir {
                        visited_dirs.push(p.to_path_buf());
                    }
                } else if p.is_file() {
                    seen += 1;
                    if decoder::is_supported_file(p) {
                        discovered.push(p.to_path_buf());
                    }
                }
            }
        } else if let Ok(rd) = std::fs::read_dir(&job.root_dir) {
            for entry in rd.flatten() {
                if job.control.cancelled.load(Ordering::Relaxed) {
                    break;
                }
                let p = entry.path();
                if p.is_file() {
                    seen += 1;
                    if decoder::is_supported_file(&p) {
                        discovered.push(p);
                    }
                }
            }
        }
        discovered
    };

    let total = files.len();
    let _ = tx_evt.send(ScanEvent::Started {
        task_id: job.task_id,
        started_at,
        thread_id,
        total,
        seen,
    });
    let _ = tx_evt.send(ScanEvent::Log(format!(
        "{} — found {total} supported file(s) ({} Q{}, {}px).",
        job.display_text,
        job.thumb_cfg.format.short_name(),
        job.thumb_cfg.quality,
        job.thumb_cfg.max_dim
    )));

    let mut indexed = 0usize;
    let mut skipped = 0usize;
    let mut done = 0usize;
    let mut dirty = 0usize;
    let mut cancelled = false;
    let mut active_ms = 0u64;
    let mut last_flush = Instant::now();
    let emit_item_refresh = job.explicit_files.is_some();

    // Parallel chunked scan: saturates CPU cores + GPU compute for every scan task
    // while keeping Pause / Resume / Cancel responsive between chunks.
    let chunk_size = if total <= 48 { 8 } else { 24 };

    for chunk in files.chunks(chunk_size) {
        let mut was_paused = false;
        while job.control.paused.load(Ordering::SeqCst) {
            if job.control.cancelled.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }
            if !was_paused {
                was_paused = true;
                let cur = chunk
                    .first()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let _ = tx_evt.send(ScanEvent::Progress {
                    task_id: job.task_id,
                    done,
                    total,
                    seen,
                    indexed,
                    skipped,
                    current_file: cur,
                    elapsed_ms: wall_start.elapsed().as_millis() as u64,
                    active_ms,
                    state: TaskState::Paused,
                });
            }
            thread::sleep(Duration::from_millis(60));
        }

        if job.control.cancelled.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }

        let work_start = Instant::now();
        if let Some(first) = chunk.first() {
            let _ = tx_evt.send(ScanEvent::Progress {
                task_id: job.task_id,
                done,
                total,
                seen,
                indexed,
                skipped,
                current_file: first.to_string_lossy().to_string(),
                elapsed_ms: wall_start.elapsed().as_millis() as u64,
                active_ms,
                state: TaskState::Running,
            });
        }

        let stats: Vec<(PathBuf, u64, String)> = chunk
            .iter()
            .map(|p| {
                let (len, mtime) = file_stat(p);
                (p.clone(), len, mtime)
            })
            .collect();

        let mut to_decode: Vec<(PathBuf, u64, String)> = Vec::new();
        if !job.force_redecode {
            if let Ok(cat) = job.db.lock() {
                for (path, len, mtime) in stats {
                    if cat.is_up_to_date(&path, len, &mtime) {
                        skipped += 1;
                        done += 1;
                    } else {
                        to_decode.push((path, len, mtime));
                    }
                }
            } else {
                to_decode = stats;
            }
        } else {
            to_decode = stats;
        }

        if !to_decode.is_empty() {
            let control_ref = &job.control;
            let decoded_opts: Vec<Option<DecodedThumbPayload>> =
                run_parallel(&to_decode, job.thumb_cfg.cpu_threads, |(path, len, mtime)| {
                    if control_ref.cancelled.load(Ordering::Relaxed) {
                        return None;
                    }
                    Some(decode_and_encode_one(
                        path,
                        *len,
                        mtime.clone(),
                        &job.gs_config,
                        &job.thumb_cfg,
                        false,
                    ))
                });

            let decoded_items: Vec<DecodedThumbPayload> =
                decoded_opts.into_iter().flatten().collect();

            let mut refreshed_kws: Vec<Vec<String>> = Vec::with_capacity(decoded_items.len());
            if let Ok(mut cat) = job.db.lock() {
                for item in &decoded_items {
                    if item.encoded_blob.is_none() {
                        let _ = tx_evt.send(ScanEvent::TaskMessage {
                            task_id: job.task_id,
                            message: format!("{}: {}", item.path.display(), item.decoded.log_line),
                            is_error: true,
                        });
                    }
                    let blob_ref = item
                        .encoded_blob
                        .as_ref()
                        .map(|(b, f)| (b.as_slice(), f.as_str()));
                    if let Err(e) = cat.upsert_thumbnail_batch(
                        &item.path,
                        &item.decoded.filetype,
                        &item.file_time_iso,
                        item.file_length,
                        item.decoded.width,
                        item.decoded.height,
                        item.decoded.color_depth,
                        item.decoded.page_count,
                        &item.decoded.keywords,
                        &item.decoded.metadata,
                        blob_ref,
                        &item.md5_hex,
                        item.visual_sig.clone(),
                    ) {
                        let _ = tx_evt.send(ScanEvent::TaskMessage {
                            task_id: job.task_id,
                            message: format!("DB write error on {}: {e}", item.path.display()),
                            is_error: true,
                        });
                        refreshed_kws.push(item.decoded.keywords.clone());
                    } else if emit_item_refresh {
                        let kws = cat
                            .get_by_path(&item.path)
                            .ok()
                            .flatten()
                            .map(|r| r.keywords)
                            .unwrap_or_else(|| item.decoded.keywords.clone());
                        refreshed_kws.push(kws);
                    } else {
                        refreshed_kws.push(Vec::new());
                    }
                    indexed += 1;
                    dirty += 1;
                }
                if dirty >= FLUSH_BATCH_THRESHOLD
                    || (dirty > 0 && last_flush.elapsed() >= Duration::from_secs(FLUSH_INTERVAL_SECS))
                {
                    let _ = cat.flush();
                    dirty = 0;
                    last_flush = Instant::now();
                }
            } else {
                for item in &decoded_items {
                    refreshed_kws.push(item.decoded.keywords.clone());
                }
            }

            if emit_item_refresh {
                for (item, merged_keywords) in
                    decoded_items.into_iter().zip(refreshed_kws.into_iter())
                {
                    let _ = tx_evt.send(ScanEvent::ItemRefreshed {
                        path: item.path,
                        filetype: item.decoded.filetype,
                        width: item.decoded.width,
                        height: item.decoded.height,
                        color_depth: item.decoded.color_depth,
                        page_count: item.decoded.page_count,
                        file_length: item.file_length,
                        file_time_iso: item.file_time_iso,
                        keywords: merged_keywords,
                        metadata: item.decoded.metadata,
                        thumb_blob: item.encoded_blob,
                        md5: item.md5_hex,
                        source_note: item.decoded.source_note,
                    });
                    done += 1;
                }
            } else {
                done += decoded_items.len();
            }
        }

        active_ms += work_start.elapsed().as_millis() as u64;
    }

    if dirty > 0 {
        if let Ok(mut cat) = job.db.lock() {
            let _ = cat.flush();
        }
    }

    let cancelled = cancelled || job.control.cancelled.load(Ordering::SeqCst);
    let scanned_dirs = if !cancelled && job.explicit_files.is_none() {
        if let Ok(mut cat) = job.db.lock() {
            let _ = cat.mark_folders_scanned(&visited_dirs);
        }
        visited_dirs
    } else {
        Vec::new()
    };

    let completed_at = chrono::Local::now().format("%m/%d/%Y  %I:%M:%S %p").to_string();
    let _ = tx_evt.send(ScanEvent::Finished {
        task_id: job.task_id,
        completed_at,
        elapsed_ms: wall_start.elapsed().as_millis() as u64,
        active_ms,
        done,
        total,
        seen,
        indexed,
        skipped,
        cancelled,
        scanned_dirs,
    });
}

pub fn file_stat(path: &Path) -> (u64, String) {
    if let Ok(md) = std::fs::metadata(path) {
        let len = md.len();
        let iso = md
            .modified()
            .ok()
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
        (len, iso)
    } else {
        (0, "1970-01-01 00:00:00".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_worker_never_drops_superseding_request() {
        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_worker_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let sample_dir = crate::sample_gen::ensure_sample_library(&tmp);
        let db = Arc::new(Mutex::new(CatalogDb::open(&tmp.join("test.tcdb")).unwrap()));

        let f1 = sample_dir.join("07_Sunset_Landscape.png");
        let f2 = sample_dir.join("10_Technical_Grid.jpg");

        let worker = BackgroundWorker::spawn();
        let make_req = |gen: u64, files: Vec<PathBuf>| WorkerRequest {
            files,
            gs_config: GhostscriptConfig {
                gs_path: String::new(),
                ..Default::default()
            },
            thumb_cfg: ThumbEncodeConfig {
                format: ThumbFormat::Webp,
                quality: 80,
                max_dim: 120,
                use_gpu: false,
                cpu_threads: 0,
            },
            db: db.clone(),
            force_redecode: true,
            generation: gen,
            send_blob_only: true,
            cache_only: false,
        };

        // Submit batch 1 (many repeats to keep worker busy), then immediately supersede with batch 2
        worker.submit(make_req(1, vec![f1.clone(); 20]));
        thread::sleep(Duration::from_millis(5));
        worker.submit(make_req(2, vec![f2.clone()]));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_gen2_done = false;
        let mut saw_gen2_item = false;
        while Instant::now() < deadline && !saw_gen2_done {
            for evt in worker.drain_events() {
                match evt {
                    WorkerEvent::ItemReady { generation: 2, path, .. } if path == f2 => {
                        saw_gen2_item = true;
                    }
                    WorkerEvent::BatchDone { generation: 2, .. } => {
                        saw_gen2_done = true;
                    }
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(saw_gen2_item, "Superseded batch 2 item was dropped!");
        assert!(saw_gen2_done, "Superseded batch 2 never finished!");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn folder_scanner_runs_multiple_scans_simultaneously_and_marks_scanned() {
        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_concurrent_scan_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir1 = tmp.join("folder_a");
        let dir2 = tmp.join("folder_b");
        let s1 = crate::sample_gen::ensure_sample_library(&dir1);
        let s2 = crate::sample_gen::ensure_sample_library(&dir2);

        let db = Arc::new(Mutex::new(CatalogDb::open(&tmp.join("test.tcdb")).unwrap()));
        let scanner = FolderScanner::spawn();

        // Pause scan 1 immediately, enqueue scan 2 right behind it:
        // With concurrent workers, scan 2 MUST finish even while scan 1 is paused!
        let ctrl1 = Arc::new(TaskControl::default());
        ctrl1.paused.store(true, Ordering::SeqCst);
        let ctrl2 = Arc::new(TaskControl::default());

        let mk_job = |task_id: u64, root: PathBuf, control: Arc<TaskControl>| FolderScanJob {
            task_id,
            display_text: format!("Scan {task_id}"),
            root_dir: root,
            explicit_files: None,
            recursive: false,
            force_redecode: false,
            gs_config: GhostscriptConfig {
                gs_path: String::new(),
                ..Default::default()
            },
            thumb_cfg: ThumbEncodeConfig {
                format: ThumbFormat::Webp,
                quality: 80,
                max_dim: 120,
                use_gpu: false,
                cpu_threads: 0,
            },
            db: db.clone(),
            control,
        };

        scanner.enqueue(mk_job(1, s1.clone(), ctrl1.clone()));
        scanner.enqueue(mk_job(2, s2.clone(), ctrl2.clone()));

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut job2_done = false;
        let mut job1_done = false;
        let mut job1_cancelled = false;

        while Instant::now() < deadline && !job2_done {
            for evt in scanner.drain_events() {
                if let ScanEvent::Finished { task_id: 2, cancelled, .. } = evt {
                    assert!(!cancelled);
                    job2_done = true;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(job2_done, "Job 2 was blocked by paused Job 1 — scans are not running simultaneously!");

        // Verify folder 2 is marked as fully scanned, while paused/cancelled folder 1 is NOT
        {
            let scanned = db.lock().unwrap().scanned_folders_set();
            assert!(scanned.contains(&crate::catalog::normalize_dir_key(&s2)));
            assert!(!scanned.contains(&crate::catalog::normalize_dir_key(&s1)));
        }

        // Now cancel paused job 1 and ensure it exits cleanly as cancelled
        ctrl1.cancelled.store(true, Ordering::SeqCst);
        ctrl1.paused.store(false, Ordering::SeqCst);
        let deadline2 = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline2 && !job1_done {
            for evt in scanner.drain_events() {
                if let ScanEvent::Finished { task_id: 1, cancelled, .. } = evt {
                    job1_done = true;
                    job1_cancelled = cancelled;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(job1_done && job1_cancelled, "Paused Job 1 did not cancel cleanly!");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
