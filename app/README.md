# ThumbCraft Studio

A fast, native desktop media browser, thumbnail cataloger, and metadata manager built in Rust with `egui`/`eframe`, GPU-accelerated `wgpu` compute downscaling, and optional Ghostscript rendering.

---

## Features

- **5-Pane Dockable Workspace** — Folder / Keyword / Gallery tree, live Preview pane, central Thumbnail Grid / Details Table / Filmstrip view, File Info & Keywords inspector, and Background Tasks & Log pane. Includes 5 layout presets and 4 color themes (*Classic Light*, *Studio Neutral*, *Darkroom*, *High Contrast*).
- **Single-File Thumbnail Catalog (`.tcdb`)** — Stores compressed thumbnails (WEBP, JXL, JPG, or PNG), perceptual similarity fingerprints, MD5 hashes, keywords, ratings, and annotations in one portable file, with on-demand **Compact Database** cleanup for dangling records and dead space.
- **Hybrid GPU + Multi-Core CPU Indexing** — Offloads large raster downscaling (`>= 0.5 MP`) to a `wgpu` WGSL compute shader while decoding smaller assets in parallel across CPU cores via `rayon`, with off-thread thumbnail texture decoding and virtualized scrolling for large folders.
- **Broad Format Support (Pure-Rust + Ghostscript)**:
  - **CorelDRAW & Corel PHOTO-PAINT**: `.cdr`, `.cmx`, `.ccx`, `.pat` (ZIP X4+ & RIFF v3–v13, legacy v2 `WL`, `@CorelBMF`, damaged-EOCD ZIP recovery, embedded keyword/XMP extraction) and `.cpt` (`CPT9FILE` / `CPT7`–`8` tiled delta-UP decompression with thumbnail + full-resolution + alpha mask support).
  - **Adobe Photoshop**: `.psd`, `.psb` (native composite & RLE/raw layer reader + XMP keywords).
  - **PostScript / PDF / Illustrator**: `.pdf`, `.ai`, `.eps`, `.epsi`, `.epsf`, `.ps`, `.prn` via Ghostscript (with configurable Thumbnail / Preview / Full-View DPI, bounding-box DPI capping for oversized artwork, and multi-page navigation).
  - **Modern & Common Raster**: `.webp`, `.avif`, `.jxl`, `.png`, `.jpg`/`.jpeg`, `.gif` (multi-frame), `.bmp`/`.dib` (including OS/2 v2 64-byte headers), `.tif`/`.tiff`, `.ico`/`.cur`, `.tga`, `.pnm`/`.ppm`/`.pgm`/`.pbm`, `.qoi`, `.hdr`, `.exr`.
  - **DirectDraw Surface (`.dds`)**: Pure-Rust decoder supporting uncompressed bitmask RGB/RGBA/Luminance/Alpha/BumpDuDv, `DXT1`–`DXT5`, `ATI1`/`BC4`, `ATI2`/`BC5`, `BC7`, DX10 extended headers, half/full float, and `YUY2`/`UYVY`.
  - **Vector**: `.svg`, `.svgz` via `resvg`.
  - **All Other File Types**: Displayed in the browser using native Windows Shell file icons without bloating the catalog database.
  - **Resilient Fallback Decoding**: Content-based magic-byte sniffing handles misnamed extensions (e.g., WebP saved as `.jpg`, PNG saved as `.webp`) and partial recovery for truncated GIF, PNG, and JPEG streams.
- **Visual Similarity & Duplicate Finding** — Perceptual 64-bit dHash + luminance + color-grid similarity search (configurable threshold & color weight), **Sort by Similarity**, and capped MD5 exact-duplicate detection across Current Folder, Current Tree, or Entire Database.
- **Keywords & Metadata** — Reads and writes keywords to both the `.tcdb` catalog and embedded file metadata (XMP / EXIF / Windows Explorer `XPKeywords` / CDR metadata). Inspects Dimensions (with Megapixels), Subsampling, Estimated Quality, Bits Per Channel, and DPI.
- **Workflow & Windows Integration** — Multi-file drag-and-drop (`CF_HDROP` via OLE `DoDragDrop`, including `Alt+Tab` handoff to other apps), multi-file Copy/Move to Folder, safe Rename (auto-selects stem and warns on extension change), instant delete (`Del` / `Ctrl+D`), live folder watcher, and an **Auto-Scan ON/OFF** toggle.

---

## How It Works

### 1. Application Architecture
The application is organized into clean Rust modules under `src/`:

| Module | Responsibility |
| --- | --- |
| `src/main.rs` | Entry point, CLI flags (`[folder]`, `--screenshot`), window/icon initialization via `eframe`. |
| `src/ui/` | `egui` immediate-mode UI: `mod.rs` (app state, event polling, caching, keyboard shortcuts), `thumb_pane.rs` (virtualized Grid/Details/Filmstrip), `tree_pane.rs`, `preview_pane.rs`, `info_pane.rs`, `tasks_pane.rs`, `dialogs.rs`, `theme.rs`. |
| `src/worker.rs` | Asynchronous background pipelines: `BackgroundWorker` (active folder indexing), `FolderScanner` (concurrent Scan Folder / Scan Tree / `F3` rebuild tasks with pause/resume/cancel), `PreviewWorker` (non-blocking full preview decoding), and `ThumbDecodePool` (parallel blob-to-RGBA decoding during scroll). |
| `src/catalog.rs` | Single-file `.tcdb` database storing thumbnail blobs, folder/file indexes, keywords, galleries, visual signatures, and compaction (`prune_dangling_records` + space reclamation). |
| `src/decoder.rs` | Format router, thumbnail encoder/decoder (`WEBP`, `JXL`, `JPG`, `PNG`), image filters, generous-limit reader, magic-byte sniffer, and truncated GIF/PNG/JPEG recovery. |
| `src/cdr.rs`, `src/cpt.rs`, `src/dds.rs`, `src/psd.rs` | Pure-Rust format parsers and decoders for CorelDRAW, Corel PHOTO-PAINT, DirectDraw Surface, and Photoshop files. |
| `src/ghostscript.rs` | Ghostscript CLI auto-detection, page counting, DPI clamping, and EPS/PDF/PS rendering. |
| `src/gpu_accel.rs` | `wgpu` compute shader downscaler with fast multi-core CPU box-filter fallback. |
| `src/image_meta.rs` | Fast header-only technical metadata probe (JPEG DQT quality & chroma subsampling, PNG/TIFF/BMP/PSD/CPT/DDS bit depth & DPI). |
| `src/similarity.rs` | Perceptual `VisualSignature` (64-bit dHash + 8×8 luma + 4×4 RGB), similarity clustering/sorting, and pure-Rust RFC 1321 MD5. |
| `src/keyword_writer.rs` | Writes keywords back into supported media files for Windows Explorer compatibility. |
| `src/drag_drop.rs`, `src/shell_icon.rs`, `src/folder_watch.rs` | Windows OLE drag-and-drop (`CF_HDROP`), Shell file icon extraction (`SHGetFileInfoW`), and live filesystem change watching. |

### 2. Data Flow & Performance Pipeline
1. **Folder Navigation**: Opening a folder populates `MediaItem` entries immediately and submits supported media paths to `BackgroundWorker` (or loads cached entries only when **Auto-Scan** is OFF).
2. **Cache Lookup vs. Decoding**: `BackgroundWorker` checks `thumbcraft.tcdb` by path, file size, and modified timestamp. Cache hits return their compressed thumbnail blob immediately; cache misses decode in parallel across CPU threads, downscale via `wgpu` (or CPU box filter), compute a `VisualSignature` and capped MD5, encode the thumbnail blob, and batch-flush to `.tcdb`.
3. **Virtualized UI Rendering**: `thumb_pane.rs` renders only visible rows (`show_rows`) and prefetches a small window of nearby thumbnails through `ThumbDecodePool`, keeping an LRU cap on live GPU textures so folders with thousands of images stay smooth and responsive.
4. **Settings & Persistence**: User settings are saved beside the executable in `thumbcraft_settings.json`, and the catalog defaults to `thumbcraft.tcdb` (configurable in **Preferences → Thumbnails & Captions**).

---

## Building From Source

### Prerequisites
- **Rust toolchain** (1.80+ recommended, edition 2021) — install via [rustup.rs](https://rustup.rs/).
- **Windows 10/11 (x86_64)** (primary target; includes native Win32 OLE drag-and-drop, Shell icons, and COFF `.rsrc` icon embedding in `build.rs` with no external `rc.exe` required).
- **Optional — GPL Ghostscript** (8.00+ / 10.x): Required only for rendering `.pdf`, `.ai`, `.eps`, `.ps`, and `.prn` files. Install from [ghostscript.com](https://www.ghostscript.com/) — ThumbCraft auto-detects `gswin64c.exe` / `gswin32c.exe` from standard install paths or `PATH`.
- **Optional — Inkscape or LibreOffice**: Can be configured in **Preferences → Ghostscript & CDR** for full vector CDR → PDF → Ghostscript rendering (native CDR embedded previews and keyword extraction work without any external tool).

### Build & Run

```powershell
# Run the full unit test suite
cargo test

# Build and run the debug build
cargo run

# Build an optimized release executable
cargo build --release
```

The release binary is created at:

```text
target\release\thumbcraft.exe
```

You can optionally pass a starting folder path on the command line:

```powershell
.\target\release\thumbcraft.exe "C:\Path\To\Your\Images"
```

---

## Keyboard Shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+Enter` | Open selected item in full-screen Lightbox viewer |
| `Shift+Enter` / Double-Click | Open selected item in default external application |
| `Alt+Enter` | Show File Info & Keywords pane |
| `F2` | Rename selected file (auto-selects filename stem) |
| `F3` | Rebuild thumbnail(s) for selected file(s) |
| `Del` / `Ctrl+D` | Delete selected file(s) |
| `Shift+Alt+F3` | Scan current folder |
| `Ctrl+Shift+F3` | Scan current folder tree recursively |
| `Alt+0` | Find Visually Similar Images |
| `Alt+Y` | Open Preferences |
| `←` / `→` / `Esc` | Navigate / close Lightbox viewer |
