use crate::cdr;
use crate::config::{GhostscriptConfig, ThumbFormat, MAX_MD5_FILE_KB};
use crate::ghostscript::{self, GsRenderMode};
use crate::psd;
use crate::similarity;
use flate2::read::GzDecoder;
use image::AnimationDecoder;
use image::{DynamicImage, ImageBuffer, Rgba};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

const MAX_MD5_BYTES: u64 = MAX_MD5_FILE_KB * 1024;

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    // CorelDRAW & Corel PHOTO-PAINT
    "cdr", "cmx", "ccx", "pat", "cpt",
    // Adobe Photoshop
    "psd", "psb",
    // PostScript / PDF / Illustrator (Ghostscript)
    "pdf", "ai", "eps", "epsi", "epsf", "ps", "prn",
    // Modern raster
    "avif", "webp", "jxl",
    // Vector
    "svg", "svgz",
    // Common raster & GIF
    "png", "jpg", "jpeg", "jpe", "gif", "bmp", "dib", "tif", "tiff", "ico", "cur", "tga", "pnm",
    "ppm", "pgm", "pbm", "qoi", "dds", "hdr", "exr",
];

pub fn is_supported_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let low = e.to_ascii_lowercase();
            SUPPORTED_EXTENSIONS.iter().any(|&ext| ext == low)
        })
        .unwrap_or(false)
}

pub fn is_common_raster_ext(ext_upper: &str) -> bool {
    matches!(
        ext_upper,
        "PNG"
            | "JPG"
            | "JPEG"
            | "JPE"
            | "BMP"
            | "DIB"
            | "TIF"
            | "TIFF"
            | "ICO"
            | "CUR"
            | "TGA"
            | "PNM"
            | "PPM"
            | "PGM"
            | "PBM"
            | "QOI"
            | "DDS"
            | "HDR"
            | "EXR"
    )
}

pub fn format_category(ext_upper: &str) -> &'static str {
    match ext_upper {
        "CDR" | "CMX" | "CCX" | "PAT" => "CorelDRAW",
        "CPT" => "Corel PHOTO-PAINT (CPT)",
        "PSD" | "PSB" => "Photoshop (PSD/PSB)",
        "GIF" => "GIF Image",
        "PDF" | "AI" | "EPS" | "EPSI" | "EPSF" | "PS" | "PRN" => "Ghostscript (PS/PDF/AI)",
        "AVIF" | "WEBP" | "JXL" => "Modern Raster",
        "SVG" | "SVGZ" => "SVG Vector",
        ext if is_common_raster_ext(ext) => "Common Raster",
        _ => "File",
    }
}

#[derive(Debug)]
pub struct DecodedMedia {
    pub image: Option<DynamicImage>,
    pub filetype: String,
    pub width: u32,
    pub height: u32,
    pub color_depth: u16,
    pub page_count: u32,
    pub keywords: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub md5: Option<String>,
    pub source_note: String,
    pub log_line: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFilter {
    None,
    Sharpen,      // 3x3 sharpen kernel: [-1,-1,-1; -1,16,-1; -1,-1,-1] / 8
    Blur,         // 3x3 Gaussian-like blur: [1,2,1; 2,4,2; 1,2,1] / 16
    EmbossMedium, // 3x3 emboss: [-1,-2,-1; 0,0,0; 1,2,1] + 192
    EdgesWeak,    // 3x3 Laplacian edge detect: [0,1,0; 1,-4,1; 0,1,0]
    Invert,       // Point invert (255 - x)
}

impl ImageFilter {
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "Original",
            Self::Sharpen => "Sharpen",
            Self::Blur => "Blur",
            Self::EmbossMedium => "Emboss Medium",
            Self::EdgesWeak => "Edges (Weak)",
            Self::Invert => "Invert",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::None,
            Self::Sharpen,
            Self::Blur,
            Self::EmbossMedium,
            Self::EdgesWeak,
            Self::Invert,
        ]
    }
}

pub fn decode_thumbnail(path: &Path, gs_config: &GhostscriptConfig) -> DecodedMedia {
    decode_media(path, gs_config, 1, GsRenderMode::Thumbnail)
}

pub fn decode_preview_page(
    path: &Path,
    gs_config: &GhostscriptConfig,
    page: usize,
    full_res: bool,
) -> DecodedMedia {
    let mode = if full_res {
        GsRenderMode::FullView
    } else {
        GsRenderMode::Preview
    };
    decode_media(path, gs_config, page, mode)
}

fn decode_media(
    path: &Path,
    gs_config: &GhostscriptConfig,
    page: usize,
    gs_mode: GsRenderMode,
) -> DecodedMedia {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let filetype = ext.to_ascii_uppercase();
    let fname = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    match ext.as_str() {
        "cdr" | "cmx" | "ccx" | "pat" => decode_cdr_file(path, &filetype, &fname, gs_config, page, gs_mode),
        "cpt" => decode_cpt_file(path, &filetype, &fname, gs_mode),
        "psd" | "psb" => decode_psd_file(path, &filetype, &fname, gs_mode),
        "gif" => decode_gif_file(path, &filetype, &fname, page),
        "pdf" | "ai" | "eps" | "epsi" | "epsf" | "ps" | "prn" => {
            decode_gs_file(path, &filetype, &fname, gs_config, page, gs_mode)
        }
        "svg" | "svgz" => decode_svg_file(path, &filetype, &fname, gs_mode),
        "avif" => decode_avif_file(path, &filetype, &fname),
        "webp" => decode_webp_file(path, &filetype, &fname),
        "jxl" => decode_jxl_file(path, &filetype, &fname),
        "dds" => decode_dds_file(path, &filetype, &fname),
        _ => decode_standard_raster(path, &filetype, &fname),
    }
}

fn decode_cdr_file(
    path: &Path,
    filetype: &str,
    fname: &str,
    gs_config: &GhostscriptConfig,
    page: usize,
    gs_mode: GsRenderMode,
) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();
    let mut page_count = 1u32;
    let mut image = None;
    let mut source_note = "CorelDRAW Parser".to_string();
    let log_line;

    let raw_bytes = std::fs::read(path);
    let md5 = raw_bytes
        .as_ref()
        .ok()
        .and_then(|b| similarity::md5_bytes_capped(b, MAX_MD5_BYTES));
    let parse_res = match raw_bytes {
        Ok(ref bytes) => cdr::parse_cdr_bytes(bytes),
        Err(ref e) => Err(format!("Failed to read CDR: {e}")),
    };

    match parse_res {
        Ok(parsed) => {
            metadata = parsed.metadata;
            keywords = parsed.keywords;
            page_count = parsed.page_count as u32;
            image = parsed.preview;

            // If full view requested and external CDR converter -> Ghostscript is configured, prefer high-res vector render
            if matches!(gs_mode, GsRenderMode::FullView) && !gs_config.cdr_converter_path.is_empty() {
                if let Ok(gs_img) =
                    ghostscript::render_cdr_via_converter_and_gs(gs_config, path, page, gs_mode)
                {
                    image = Some(gs_img);
                    source_note = "CDR -> Ghostscript Vector".to_string();
                }
            }

            // If no embedded preview was found, try embedded PS/PDF via Ghostscript, then external CDR converter -> Ghostscript
            if image.is_none() {
                if let Some((ref ps_bytes, ps_ext)) = parsed.embedded_ps_pdf {
                    if let Ok(gs_img) =
                        ghostscript::render_bytes(gs_config, ps_bytes, ps_ext, page, gs_mode)
                    {
                        image = Some(gs_img);
                        source_note = "CDR Embedded PS/PDF via Ghostscript".to_string();
                    }
                }
            }
            if image.is_none() && !gs_config.cdr_converter_path.is_empty() {
                if let Ok(gs_img) =
                    ghostscript::render_cdr_via_converter_and_gs(gs_config, path, page, gs_mode)
                {
                    image = Some(gs_img);
                    source_note = "CDR Converter + Ghostscript".to_string();
                }
            }

            if image.is_some() && source_note == "CorelDRAW Parser" {
                source_note = format!("{} Preview", parsed.version_label);
            }

            log_line = format!(
                "CDR {} — {} | {} keyword(s) extracted",
                fname,
                parsed.version_label,
                keywords.len()
            );
        }
        Err(e) => {
            // Even if container didn't match standard CDR headers, try Ghostscript directly (some .cdr exports are PS/PDF)
            // and universal magic-byte decode (in case a raster image was renamed .cdr).
            if let Ok(gs_img) = ghostscript::render_page(gs_config, path, page, gs_mode) {
                image = Some(gs_img);
                source_note = "Ghostscript Direct".to_string();
                log_line = format!("CDR {} rendered directly via Ghostscript", fname);
            } else if let Some((img, kind)) = raw_bytes.as_ref().ok().and_then(|b| decode_by_magic_bytes(b)) {
                image = Some(img);
                source_note = format!("CDR Fallback ({kind})");
                log_line = format!("CDR {} decoded as {kind}", fname);
            } else {
                metadata.insert("Decode Note".into(), e.clone());
                source_note = "CDR (No Preview)".to_string();
                log_line = format!("CDR {} parse warning: {}", fname, e);
            }
        }
    }

    let (width, height) = image
        .as_ref()
        .map(|i| (i.width(), i.height()))
        .unwrap_or((0, 0));

    metadata
        .entry(crate::image_meta::KEY_SUBSAMPLING.into())
        .or_insert_with(|| "None (Vector)".into());
    metadata
        .entry(crate::image_meta::KEY_ESTIMATED_QUALITY.into())
        .or_insert_with(|| "Vector".into());
    metadata
        .entry(crate::image_meta::KEY_BITS_PER_CHANNEL.into())
        .or_insert_with(|| "8-bit".into());

    DecodedMedia {
        image,
        filetype: filetype.to_string(),
        width,
        height,
        color_depth: 24,
        page_count,
        keywords,
        metadata,
        md5,
        source_note,
        log_line,
    }
}

fn decode_gs_file(
    path: &Path,
    filetype: &str,
    fname: &str,
    gs_config: &GhostscriptConfig,
    page: usize,
    gs_mode: GsRenderMode,
) -> DecodedMedia {
    let (keywords, mut metadata) = ghostscript::extract_ps_pdf_metadata(path);
    let page_count = ghostscript::count_pages(gs_config, path) as u32;
    metadata.insert("Pages".into(), page_count.to_string());

    let dpi_used = match gs_mode {
        GsRenderMode::Thumbnail => ghostscript::clamp_dpi(gs_config.thumbnail_dpi),
        GsRenderMode::Preview => ghostscript::clamp_dpi(gs_config.preview_dpi),
        GsRenderMode::FullView => ghostscript::clamp_dpi(gs_config.full_view_dpi),
    };
    if let Some(sig) = ghostscript::identify_ps_pdf_header(path) {
        metadata.insert("PS/PDF Format".into(), sig);
    }
    let ppm = ghostscript::dpi_to_pixels_per_meter(dpi_used);
    metadata.insert(
        "Render Engine".into(),
        format!("Ghostscript ({dpi_used} DPI, {ppm} px/m)"),
    );
    metadata.insert(crate::image_meta::KEY_SUBSAMPLING.into(), "None (Vector)".into());
    metadata.insert(crate::image_meta::KEY_ESTIMATED_QUALITY.into(), "Vector".into());
    metadata.insert(
        crate::image_meta::KEY_BITS_PER_CHANNEL.into(),
        if gs_config.color_depth == 1 {
            "1-bit".into()
        } else {
            "8-bit".into()
        },
    );
    metadata.insert(crate::image_meta::KEY_DPI.into(), format!("{dpi_used} DPI"));

    let (image, source_note, log_line) =
        match ghostscript::render_page(gs_config, path, page, gs_mode) {
            Ok(img) => (
                Some(img),
                format!("Ghostscript {} DPI", dpi_used),
                format!(
                    "Ghostscript rendered {} (p.{}/{}, {} DPI, {} kw)",
                    fname,
                    page,
                    page_count,
                    dpi_used,
                    keywords.len()
                ),
            ),
            Err(err) => {
                metadata.insert("Ghostscript Error".into(), err.clone());
                (
                    None,
                    "Ghostscript Error".to_string(),
                    format!("Ghostscript error on {}: {}", fname, err),
                )
            }
        };

    let (width, height) = image
        .as_ref()
        .map(|i| (i.width(), i.height()))
        .unwrap_or((0, 0));

    DecodedMedia {
        image,
        filetype: filetype.to_string(),
        width,
        height,
        color_depth: gs_config.color_depth as u16,
        page_count,
        keywords,
        metadata,
        md5: None,
        source_note,
        log_line,
    }
}

fn decode_svg_file(
    path: &Path,
    filetype: &str,
    fname: &str,
    gs_mode: GsRenderMode,
) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return error_media(filetype, format!("Read error: {e}"));
        }
    };
    let md5 = similarity::md5_bytes_capped(&raw, MAX_MD5_BYTES);

    let xml_bytes = if filetype == "SVGZ" || raw.starts_with(&[0x1f, 0x8b]) {
        let mut dec = GzDecoder::new(&raw[..]);
        let mut out = Vec::new();
        if dec.read_to_end(&mut out).is_ok() {
            out
        } else {
            raw
        }
    } else {
        raw
    };

    let xml_text = String::from_utf8_lossy(&xml_bytes);
    let (kws, meta) = cdr::parse_xmp_or_xml_metadata(&xml_text);
    keywords.extend(kws);
    metadata.extend(meta);
    metadata.insert("Render Engine".into(), "Native SVG (resvg)".into());
    metadata.insert(crate::image_meta::KEY_SUBSAMPLING.into(), "None (Vector)".into());
    metadata.insert(
        crate::image_meta::KEY_ESTIMATED_QUALITY.into(),
        "100% (Vector)".into(),
    );
    metadata.insert(crate::image_meta::KEY_BITS_PER_CHANNEL.into(), "8-bit".into());
    metadata.insert(crate::image_meta::KEY_DPI.into(), "96 DPI".into());

    let opt = resvg::usvg::Options::default();
    let tree = match resvg::usvg::Tree::from_data(&xml_bytes, &opt) {
        Ok(t) => t,
        Err(e) => {
            metadata.insert("SVG Error".into(), e.to_string());
            return DecodedMedia {
                image: None,
                filetype: filetype.to_string(),
                width: 0,
                height: 0,
                color_depth: 32,
                page_count: 1,
                keywords,
                metadata,
                md5,
                source_note: "SVG Error".into(),
                log_line: format!("SVG parse failed for {fname}: {e}"),
            };
        }
    };

    let size = tree.size();
    let src_w = size.width().max(1.0);
    let src_h = size.height().max(1.0);
    let target_max = match gs_mode {
        GsRenderMode::Thumbnail => 256.0f32,
        GsRenderMode::Preview => 640.0f32,
        GsRenderMode::FullView => 1200.0f32,
    };
    let scale = (target_max / src_w.max(src_h)).min(4.0).max(0.25);
    let pw = (src_w * scale).round().max(1.0) as u32;
    let ph = (src_h * scale).round().max(1.0) as u32;

    let mut image = None;
    if let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(pw, ph) {
        let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        // Convert premultiplied RGBA to straight RGBA
        let mut straight = pixmap.take();
        for px in straight.chunks_exact_mut(4) {
            let a = px[3];
            if a > 0 && a < 255 {
                let af = a as f32 / 255.0;
                px[0] = ((px[0] as f32 / af).min(255.0)) as u8;
                px[1] = ((px[1] as f32 / af).min(255.0)) as u8;
                px[2] = ((px[2] as f32 / af).min(255.0)) as u8;
            }
        }
        if let Some(buf) = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(pw, ph, straight) {
            image = Some(DynamicImage::ImageRgba8(buf));
        }
    }

    DecodedMedia {
        image,
        filetype: filetype.to_string(),
        width: src_w.round() as u32,
        height: src_h.round() as u32,
        color_depth: 32,
        page_count: 1,
        keywords,
        metadata,
        md5,
        source_note: "Native SVG".into(),
        log_line: format!("SVG {fname} rasterized ({pw}x{ph})"),
    }
}

fn decode_avif_file(path: &Path, filetype: &str, fname: &str) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return error_media(filetype, format!("Read error: {e}")),
    };
    let md5 = similarity::md5_bytes_capped(&bytes, MAX_MD5_BYTES);

    if let Some((kws, meta)) = cdr::extract_embedded_xmp(&bytes) {
        keywords.extend(kws);
        metadata.extend(meta);
    }
    metadata.insert("Render Engine".into(), "Pure-Rust AVIF (oxideav-avif)".into());
    crate::image_meta::inspect_bytes_technical_metadata(&bytes, "avif").insert_into(&mut metadata);

    match oxideav_avif::decode(&bytes) {
        Ok(decoded) => {
            let w = decoded.width();
            let h = decoded.height();
            let rgba = decoded.to_rgba8();
            let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, rgba)
                .map(DynamicImage::ImageRgba8);
            DecodedMedia {
                image: img,
                filetype: filetype.to_string(),
                width: w,
                height: h,
                color_depth: 32,
                page_count: 1,
                keywords,
                metadata,
                md5,
                source_note: "AVIF Native".into(),
                log_line: format!("AVIF {fname} decoded ({w}x{h})"),
            }
        }
        Err(e) => {
            if let Some((img, kind)) = decode_by_magic_bytes(&bytes) {
                let (w, h) = (img.width(), img.height());
                let bpp = img.color().bits_per_pixel();
                metadata.insert("Actual Format".into(), kind.into());
                return DecodedMedia {
                    image: Some(img),
                    filetype: filetype.to_string(),
                    width: w,
                    height: h,
                    color_depth: bpp,
                    page_count: 1,
                    keywords,
                    metadata,
                    md5,
                    source_note: format!("{kind} (as AVIF)"),
                    log_line: format!("AVIF {fname} decoded via {kind} ({w}x{h})"),
                };
            }
            metadata.insert("AVIF Error".into(), format!("{e:?}"));
            DecodedMedia {
                image: None,
                filetype: filetype.to_string(),
                width: 0,
                height: 0,
                color_depth: 24,
                page_count: 1,
                keywords,
                metadata,
                md5,
                source_note: "AVIF Error".into(),
                log_line: format!("AVIF decode failed for {fname}: {e:?}"),
            }
        }
    }
}

fn decode_webp_file(path: &Path, filetype: &str, fname: &str) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return error_media(filetype, format!("Read error: {e}")),
    };
    let md5 = similarity::md5_bytes_capped(&bytes, MAX_MD5_BYTES);

    if let Some((kws, meta)) = cdr::extract_embedded_xmp(&bytes) {
        keywords.extend(kws);
        metadata.extend(meta);
    }
    extract_exif_metadata(&bytes, &mut keywords, &mut metadata);
    cdr::dedup_keywords(&mut keywords);
    metadata.insert("Render Engine".into(), "Pure-Rust WEBP (oxideav-webp)".into());
    crate::image_meta::inspect_bytes_technical_metadata(&bytes, "webp").insert_into(&mut metadata);

    match oxideav_webp::decode_rgba8(&bytes) {
        Ok(img) => {
            let (w, h) = (img.width, img.height);
            let dyn_img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, img.data)
                .map(DynamicImage::ImageRgba8);
            DecodedMedia {
                image: dyn_img,
                filetype: filetype.to_string(),
                width: w,
                height: h,
                color_depth: 32,
                page_count: 1,
                keywords,
                metadata,
                md5,
                source_note: "WEBP Native".into(),
                log_line: format!("WEBP {fname} decoded ({w}x{h})"),
            }
        }
        Err(e) => {
            // Many web-saved .webp files are actually PNG/JPEG/GIF/AVIF with a .webp extension
            if let Some((img, kind)) = decode_by_magic_bytes(&bytes) {
                let (w, h) = (img.width(), img.height());
                let bpp = img.color().bits_per_pixel();
                metadata.insert("Actual Format".into(), kind.into());
                metadata.insert("Render Engine".into(), format!("{kind} (magic-sniffed)"));
                return DecodedMedia {
                    image: Some(img),
                    filetype: filetype.to_string(),
                    width: w,
                    height: h,
                    color_depth: bpp,
                    page_count: 1,
                    keywords,
                    metadata,
                    md5,
                    source_note: format!("{kind} (as WEBP)"),
                    log_line: format!("WEBP {fname} decoded via {kind} ({w}x{h})"),
                };
            }
            error_media(filetype, format!("WEBP decode error: {e:?}"))
        }
    }
}

fn decode_jxl_file(path: &Path, filetype: &str, fname: &str) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return error_media(filetype, format!("Read error: {e}")),
    };
    let md5 = similarity::md5_bytes_capped(&bytes, MAX_MD5_BYTES);

    if let Some((kws, meta)) = cdr::extract_embedded_xmp(&bytes) {
        keywords.extend(kws);
        metadata.extend(meta);
    }
    metadata.insert("Render Engine".into(), "Pure-Rust JXL (jxl-oxide)".into());
    crate::image_meta::inspect_bytes_technical_metadata(&bytes, "jxl").insert_into(&mut metadata);

    match decode_jxl_bytes(&bytes) {
        Some(img) => {
            let (w, h) = (img.width(), img.height());
            DecodedMedia {
                image: Some(img),
                filetype: filetype.to_string(),
                width: w,
                height: h,
                color_depth: 32,
                page_count: 1,
                keywords,
                metadata,
                md5,
                source_note: "JXL Native".into(),
                log_line: format!("JXL {fname} decoded ({w}x{h})"),
            }
        }
        None => {
            if let Some((img, kind)) = decode_by_magic_bytes(&bytes) {
                let (w, h) = (img.width(), img.height());
                let bpp = img.color().bits_per_pixel();
                metadata.insert("Actual Format".into(), kind.into());
                return DecodedMedia {
                    image: Some(img),
                    filetype: filetype.to_string(),
                    width: w,
                    height: h,
                    color_depth: bpp,
                    page_count: 1,
                    keywords,
                    metadata,
                    md5,
                    source_note: format!("{kind} (as JXL)"),
                    log_line: format!("JXL {fname} decoded via {kind} ({w}x{h})"),
                };
            }
            error_media(filetype, "JXL decode error".to_string())
        }
    }
}

fn decode_dds_file(path: &Path, filetype: &str, fname: &str) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return error_media(filetype, format!("Read error: {e}")),
    };
    let md5 = similarity::md5_bytes_capped(&bytes, MAX_MD5_BYTES);

    if let Some((kws, meta)) = cdr::extract_embedded_xmp(&bytes) {
        keywords.extend(kws);
        metadata.extend(meta);
    }
    cdr::dedup_keywords(&mut keywords);
    crate::image_meta::inspect_bytes_technical_metadata(&bytes, "dds").insert_into(&mut metadata);

    match crate::dds::decode_dds_bytes(&bytes) {
        Ok(decoded) => {
            let w = decoded.info.width;
            let h = decoded.info.height;
            metadata.insert("DDS Format".into(), decoded.info.format_label.clone());
            metadata.insert("Mipmaps".into(), decoded.info.mipmap_count.to_string());
            metadata.insert(
                "Render Engine".into(),
                format!("Native DDS ({})", decoded.info.format_label),
            );
            metadata
                .entry(crate::image_meta::KEY_BITS_PER_CHANNEL.into())
                .or_insert_with(|| decoded.info.bits_per_channel.clone());
            DecodedMedia {
                image: Some(decoded.image),
                filetype: filetype.to_string(),
                width: w,
                height: h,
                color_depth: decoded.info.color_depth,
                page_count: 1,
                keywords,
                metadata,
                md5,
                source_note: format!("DDS ({})", decoded.info.format_label),
                log_line: format!(
                    "DDS {fname} decoded ({w}x{h}, {})",
                    decoded.info.format_label
                ),
            }
        }
        Err(native_err) => {
            // Fallback to image crate in case of an exotic variant it happens to support
            if let Ok(img) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Dds) {
                let w = img.width();
                let h = img.height();
                let bpp = img.color().bits_per_pixel();
                metadata.insert("Render Engine".into(), "Native Raster (DDS)".into());
                DecodedMedia {
                    image: Some(img),
                    filetype: filetype.to_string(),
                    width: w,
                    height: h,
                    color_depth: bpp,
                    page_count: 1,
                    keywords,
                    metadata,
                    md5,
                    source_note: "DDS Fallback".into(),
                    log_line: format!("DDS {fname} decoded ({w}x{h})"),
                }
            } else if let Some((img, kind)) = decode_by_magic_bytes(&bytes) {
                let (w, h) = (img.width(), img.height());
                let bpp = img.color().bits_per_pixel();
                metadata.insert("Actual Format".into(), kind.into());
                DecodedMedia {
                    image: Some(img),
                    filetype: filetype.to_string(),
                    width: w,
                    height: h,
                    color_depth: bpp,
                    page_count: 1,
                    keywords,
                    metadata,
                    md5,
                    source_note: format!("{kind} (as DDS)"),
                    log_line: format!("DDS {fname} decoded via {kind} ({w}x{h})"),
                }
            } else {
                error_media(filetype, format!("DDS decode error: {native_err}"))
            }
        }
    }
}

fn decode_cpt_file(
    path: &Path,
    filetype: &str,
    fname: &str,
    gs_mode: GsRenderMode,
) -> DecodedMedia {
    let prefer_fast_thumb = matches!(gs_mode, GsRenderMode::Thumbnail);
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return error_media(filetype, format!("Read error: {e}")),
    };
    let md5 = similarity::md5_bytes_capped(&bytes, MAX_MD5_BYTES);
    let mut keywords = Vec::new();
    let mut extra_meta = BTreeMap::new();
    if let Some((kws, meta)) = cdr::extract_embedded_xmp(&bytes) {
        keywords.extend(kws);
        extra_meta.extend(meta);
    }
    cdr::dedup_keywords(&mut keywords);

    match crate::cpt::parse_cpt_bytes(&bytes, prefer_fast_thumb) {
        Ok(mut parsed) => {
            for (k, v) in extra_meta {
                parsed.metadata.entry(k).or_insert(v);
            }
            crate::image_meta::inspect_bytes_technical_metadata(&bytes, "cpt")
                .insert_into(&mut parsed.metadata);
            parsed
                .metadata
                .insert("Render Engine".into(), parsed.source_note.clone());
            let (w, h) = if parsed.info.width > 0 && parsed.info.height > 0 {
                (parsed.info.width, parsed.info.height)
            } else {
                parsed
                    .image
                    .as_ref()
                    .map(|i| (i.width(), i.height()))
                    .unwrap_or((0, 0))
            };
            let log_line = format!(
                "CPT {fname}: {} ({w}x{h})",
                parsed.source_note
            );
            DecodedMedia {
                image: parsed.image,
                filetype: filetype.to_string(),
                width: w,
                height: h,
                color_depth: parsed.info.color_depth,
                page_count: 1,
                keywords,
                metadata: parsed.metadata,
                md5,
                source_note: parsed.source_note,
                log_line,
            }
        }
        Err(e) => {
            if let Some((img, kind)) = decode_by_magic_bytes(&bytes) {
                let (w, h) = (img.width(), img.height());
                let bpp = img.color().bits_per_pixel();
                extra_meta.insert("Actual Format".into(), kind.into());
                return DecodedMedia {
                    image: Some(img),
                    filetype: filetype.to_string(),
                    width: w,
                    height: h,
                    color_depth: bpp,
                    page_count: 1,
                    keywords,
                    metadata: extra_meta,
                    md5,
                    source_note: format!("{kind} (as CPT)"),
                    log_line: format!("CPT {fname} decoded via {kind} ({w}x{h})"),
                };
            }
            error_media(filetype, format!("CPT decode error: {e}"))
        }
    }
}

fn decode_psd_file(
    path: &Path,
    filetype: &str,
    fname: &str,
    gs_mode: GsRenderMode,
) -> DecodedMedia {
    let prefer_fast_thumb = matches!(gs_mode, GsRenderMode::Thumbnail);
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return error_media(filetype, format!("Read error: {e}")),
    };
    let md5 = similarity::md5_bytes_capped(&bytes, MAX_MD5_BYTES);
    match psd::parse_psd_bytes(&bytes, prefer_fast_thumb) {
        Ok(mut parsed) => {
            crate::image_meta::inspect_bytes_technical_metadata(&bytes, "psd")
                .insert_into(&mut parsed.metadata);
            let color_depth = parsed.depth.saturating_mul(parsed.channels.min(4));
            let log_line = format!(
                "{filetype} {fname}: {} ({}x{}, {} keywords)",
                parsed.source_note,
                parsed.width,
                parsed.height,
                parsed.keywords.len()
            );
            DecodedMedia {
                image: parsed.image,
                filetype: filetype.to_string(),
                width: parsed.width,
                height: parsed.height,
                color_depth,
                page_count: 1,
                keywords: parsed.keywords,
                metadata: parsed.metadata,
                md5,
                source_note: parsed.source_note,
                log_line,
            }
        }
        Err(e) => error_media(filetype, format!("{filetype} decode error: {e}")),
    }
}

fn decode_gif_file(path: &Path, filetype: &str, fname: &str, page: usize) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return error_media(filetype, format!("Read error: {e}")),
    };
    let md5 = similarity::md5_bytes_capped(&bytes, MAX_MD5_BYTES);

    if let Some((kws, meta)) = cdr::extract_embedded_xmp(&bytes) {
        keywords.extend(kws);
        metadata.extend(meta);
    }
    cdr::dedup_keywords(&mut keywords);
    metadata.insert("Render Engine".into(), "Native GIF".into());
    crate::image_meta::inspect_bytes_technical_metadata(&bytes, "gif").insert_into(&mut metadata);

    let cursor = std::io::Cursor::new(&bytes);
    if let Ok(decoder) = image::codecs::gif::GifDecoder::new(cursor) {
        let mut frame_count = 0u32;
        let target_idx = page.saturating_sub(1);
        let mut chosen: Option<DynamicImage> = None;
        let mut first_frame: Option<DynamicImage> = None;

        for (i, frame_res) in decoder.into_frames().enumerate() {
            let Ok(frame) = frame_res else {
                break;
            };
            frame_count += 1;
            if i == 0 || i == target_idx {
                let img = DynamicImage::ImageRgba8(frame.into_buffer());
                if i == 0 {
                    first_frame = Some(img.clone());
                }
                if i == target_idx {
                    chosen = Some(img);
                }
            }
        }

        let image = chosen.or(first_frame);
        if let Some(img) = image {
            let (w, h) = (img.width(), img.height());
            let pages = frame_count.max(1);
            metadata.insert("Frames".into(), pages.to_string());
            metadata.insert(
                "Animation".into(),
                if pages > 1 {
                    format!("Animated GIF ({pages} frames)")
                } else {
                    "Static GIF".into()
                },
            );
            return DecodedMedia {
                image: Some(img),
                filetype: filetype.to_string(),
                width: w,
                height: h,
                color_depth: 8,
                page_count: pages,
                keywords,
                metadata,
                md5,
                source_note: if pages > 1 {
                    format!("Animated GIF ({pages}f)")
                } else {
                    "Native GIF".into()
                },
                log_line: format!("GIF {fname} decoded ({w}x{h}, {pages} frame(s))"),
            };
        }
    }

    // Fallback 1: tolerant single-frame GIF LZW recovery (handles truncated GIF sub-blocks)
    if let Some(img) = decode_truncated_gif(&bytes) {
        let (w, h) = (img.width(), img.height());
        metadata.insert("Decode Note".into(), "Recovered truncated GIF stream".into());
        return DecodedMedia {
            image: Some(img),
            filetype: filetype.to_string(),
            width: w,
            height: h,
            color_depth: 8,
            page_count: 1,
            keywords,
            metadata,
            md5,
            source_note: "GIF (Recovered)".into(),
            log_line: format!("GIF {fname} recovered ({w}x{h})"),
        };
    }

    // Fallback 2: magic-byte detection (handles WebP/PNG/JPEG/BMP/ZIP-CDR misnamed as .gif)
    if let Some((img, kind)) = decode_by_magic_bytes(&bytes) {
        let (w, h) = (img.width(), img.height());
        let bpp = img.color().bits_per_pixel();
        metadata.insert("Actual Format".into(), kind.into());
        return DecodedMedia {
            image: Some(img),
            filetype: filetype.to_string(),
            width: w,
            height: h,
            color_depth: bpp,
            page_count: 1,
            keywords,
            metadata,
            md5,
            source_note: format!("{kind} (as GIF)"),
            log_line: format!("GIF {fname} decoded via {kind} ({w}x{h})"),
        };
    }

    error_media(filetype, "GIF decode error: The image format could not be determined".into())
}

fn decode_standard_raster(path: &Path, filetype: &str, fname: &str) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();

    // Single-pass disk read: extract XMP, EXIF (including Windows Explorer XPKeywords),
    // compute capped MD5, and decode the image directly from memory without re-reading disk.
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return error_media(filetype, format!("Read error: {e}")),
    };
    let md5 = similarity::md5_bytes_capped(&bytes, MAX_MD5_BYTES);

    if let Some((kws, meta)) = cdr::extract_embedded_xmp(&bytes) {
        keywords.extend(kws);
        metadata.extend(meta);
    }
    extract_exif_metadata(&bytes, &mut keywords, &mut metadata);
    cdr::dedup_keywords(&mut keywords);
    let ext_low = filetype.to_ascii_lowercase();
    crate::image_meta::inspect_bytes_technical_metadata(&bytes, &ext_low).insert_into(&mut metadata);

    // 1. Try extension-hinted decode with generous memory limits (2 GiB)
    let fmt_hint = image::ImageFormat::from_path(path).ok();
    let first_err: Option<image::ImageError>;
    let mut decoded_res: Option<(DynamicImage, String)> = match fmt_hint {
        Some(fmt) => match decode_image_with_limits(&bytes, Some(fmt)) {
            Ok(img) => {
                first_err = None;
                Some((img, "Native Raster".to_string()))
            }
            Err(e) => {
                first_err = Some(e);
                None
            }
        },
        None => {
            first_err = None;
            None
        }
    };

    // 2. Fallback to universal magic-byte sniffer & tolerant decoders
    // (handles WebP/AVIF/JXL/CDR-ZIP misnamed as .jpg/.png/.bmp, OS/2 v2 64-byte BMPs,
    // truncated PNGs, truncated JPEGs, truncated GIFs, etc.)
    if decoded_res.is_none() {
        if let Some((img, kind)) = decode_by_magic_bytes(&bytes) {
            metadata.insert("Actual Format".into(), kind.into());
            decoded_res = Some((img, format!("Native ({kind})")));
        }
    }

    match decoded_res {
        Some((img, engine)) => {
            let w = img.width();
            let h = img.height();
            let bpp = img.color().bits_per_pixel();
            metadata
                .entry(crate::image_meta::KEY_BITS_PER_CHANNEL.into())
                .or_insert_with(|| {
                    crate::image_meta::bits_per_channel_from_color_type(img.color()).into()
                });
            metadata.insert("Render Engine".into(), engine.clone());
            DecodedMedia {
                image: Some(img),
                filetype: filetype.to_string(),
                width: w,
                height: h,
                color_depth: bpp,
                page_count: 1,
                keywords,
                metadata,
                md5,
                source_note: engine,
                log_line: format!("{filetype} {fname} decoded ({w}x{h})"),
            }
        }
        None => {
            let err_msg = first_err
                .map(|e| format!("Decode error: {e}"))
                .unwrap_or_else(|| "Decode error: Unrecognized or corrupt image format".into());
            error_media(filetype, err_msg)
        }
    }
}

// ---------------------------------------------------------------------------
// Generous-limit Image Reader, Magic-Byte Dispatcher & Tolerant Fallbacks
// ---------------------------------------------------------------------------

/// Decodes an image from memory with raised memory limits (2 GiB / 65536 px side) so huge
/// print-resolution JPEGs and Ghostscript PNG renders never trip image-rs's default 512 MiB cap.
pub fn decode_image_with_limits(
    bytes: &[u8],
    format_hint: Option<image::ImageFormat>,
) -> image::ImageResult<DynamicImage> {
    let mut reader = match format_hint {
        Some(fmt) => image::ImageReader::with_format(std::io::Cursor::new(bytes), fmt),
        None => image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(image::ImageError::IoError)?,
    };
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(65_536);
    limits.max_image_height = Some(65_536);
    limits.max_alloc = Some(2 * 1024 * 1024 * 1024);
    reader.limits(limits);
    reader.decode()
}

/// Inspects actual file header magic bytes and decodes regardless of file extension,
/// including WEBP, AVIF, JXL, DDS, PSD, OS/2 v2 BMP, ZIP-CDR / zipped images, SVG,
/// and tolerant recovery for truncated GIF / PNG / JPEG streams.
pub fn decode_by_magic_bytes(bytes: &[u8]) -> Option<(DynamicImage, &'static str)> {
    if bytes.len() < 4 {
        return None;
    }

    // 1. WEBP (RIFF....WEBP) — very commonly saved with .jpg / .png extensions from the web
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        if let Ok(img) = oxideav_webp::decode_rgba8(bytes) {
            if let Some(buf) = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(img.width, img.height, img.data) {
                return Some((DynamicImage::ImageRgba8(buf), "WEBP"));
            }
        }
    }

    // 2. PNG (\x89PNG\r\n\x1A\n) + truncated deflate recovery
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        if let Ok(img) = decode_image_with_limits(bytes, Some(image::ImageFormat::Png)) {
            return Some((img, "PNG"));
        }
        if let Some(img) = decode_truncated_png(bytes) {
            return Some((img, "PNG (Recovered)"));
        }
    }

    // 3. JPEG (\xFF\xD8\xFF) + truncated stream recovery
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        if let Ok(img) = decode_image_with_limits(bytes, Some(image::ImageFormat::Jpeg)) {
            return Some((img, "JPEG"));
        }
        if let Some(img) = decode_truncated_jpeg(bytes) {
            return Some((img, "JPEG (Recovered)"));
        }
    }

    // 4. GIF (GIF87a / GIF89a) + truncated sub-block LZW recovery
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        if let Ok(img) = decode_image_with_limits(bytes, Some(image::ImageFormat::Gif)) {
            return Some((img, "GIF"));
        }
        if let Some(img) = decode_truncated_gif(bytes) {
            return Some((img, "GIF (Recovered)"));
        }
    }

    // 5. BMP ("BM") including OS/2 v2 64-byte & 16-byte headers
    if bytes.starts_with(b"BM") {
        if let Some(img) = decode_bmp_tolerant(bytes) {
            return Some((img, "BMP"));
        }
    }

    // 6. AVIF / HEIF ISO-BMFF (....ftyp)
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        if let Ok(decoded) = oxideav_avif::decode(bytes) {
            let (w, h) = (decoded.width(), decoded.height());
            if let Some(buf) = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, decoded.to_rgba8()) {
                return Some((DynamicImage::ImageRgba8(buf), "AVIF"));
            }
        }
    }

    // 7. JPEG XL
    if bytes.starts_with(&[0xFF, 0x0A])
        || bytes.starts_with(&[0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' '])
    {
        if let Some(img) = decode_jxl_bytes(bytes) {
            return Some((img, "JXL"));
        }
    }

    // 8. DDS
    if bytes.starts_with(b"DDS ") {
        if let Ok(dec) = crate::dds::decode_dds_bytes(bytes) {
            return Some((dec.image, "DDS"));
        }
    }

    // 9. Photoshop PSD/PSB
    if bytes.starts_with(b"8BPS") {
        if let Ok(parsed) = psd::parse_psd_bytes(bytes, true) {
            if let Some(img) = parsed.image {
                return Some((img, "PSD"));
            }
        }
    }

    // 9b. Corel PHOTO-PAINT CPT
    if crate::cpt::is_cpt_magic(bytes) {
        if let Ok(parsed) = crate::cpt::parse_cpt_bytes(bytes, true) {
            if let Some(img) = parsed.image {
                return Some((img, "CPT"));
            }
        }
    }

    // 10. TIFF
    if &bytes[0..4] == b"II*\0" || &bytes[0..4] == b"MM\0*" {
        if let Ok(img) = decode_image_with_limits(bytes, Some(image::ImageFormat::Tiff)) {
            return Some((img, "TIFF"));
        }
    }

    // 11. ZIP-based container (PK\x03\x04): CorelDRAW X4+ or zipped image renamed as .jpg/.gif/.bmp
    if bytes.starts_with(b"PK\x03\x04") {
        if let Ok(parsed) = cdr::parse_cdr_bytes(bytes) {
            if let Some(img) = parsed.preview {
                return Some((img, "ZIP/CDR"));
            }
        }
        if let Some(img) = decode_first_image_in_zip(bytes) {
            return Some((img, "ZIP Image"));
        }
    }

    // 12. RIFF CorelDRAW/CMX
    if bytes.starts_with(b"RIFF") {
        if let Ok(parsed) = cdr::parse_cdr_bytes(bytes) {
            if let Some(img) = parsed.preview {
                return Some((img, "RIFF/CDR"));
            }
        }
    }

    // 13. SVG XML misnamed with a raster extension
    let trimmed = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    if bytes[trimmed..].starts_with(b"<svg") || bytes[trimmed..].starts_with(b"<?xml") {
        let opt = resvg::usvg::Options::default();
        if let Ok(tree) = resvg::usvg::Tree::from_data(&bytes[trimmed..], &opt) {
            let sz = tree.size();
            let sw = sz.width().max(1.0);
            let sh = sz.height().max(1.0);
            let scale = (512.0 / sw.max(sh)).min(4.0).max(0.25);
            let pw = (sw * scale).round().max(1.0) as u32;
            let ph = (sh * scale).round().max(1.0) as u32;
            if let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(pw, ph) {
                resvg::render(
                    &tree,
                    resvg::tiny_skia::Transform::from_scale(scale, scale),
                    &mut pixmap.as_mut(),
                );
                if let Some(buf) = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(pw, ph, pixmap.take()) {
                    return Some((DynamicImage::ImageRgba8(buf), "SVG"));
                }
            }
        }
    }

    // 14. Generic guessed format with generous limits
    if let Ok(img) = decode_image_with_limits(bytes, None) {
        return Some((img, "Raster"));
    }

    None
}

fn decode_first_image_in_zip(bytes: &[u8]) -> Option<DynamicImage> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    for i in 0..archive.len().min(64) {
        let Ok(mut f) = archive.by_index(i) else {
            continue;
        };
        if f.is_dir() || f.size() == 0 || f.size() > 64 * 1024 * 1024 {
            continue;
        }
        let lower = f.name().to_ascii_lowercase();
        if lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".bmp")
            || lower.ends_with(".gif")
            || lower.ends_with(".webp")
            || lower.ends_with(".tif")
            || lower.ends_with(".tiff")
        {
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_ok() {
                if !buf.starts_with(b"PK\x03\x04") {
                    if let Some((img, _)) = decode_by_magic_bytes(&buf) {
                        return Some(img);
                    }
                }
            }
        }
    }
    None
}

/// Decodes BMP files including OS/2 2.x 64-byte (`BITMAPINFOHEADER2`) and 16-byte DIB headers
/// which `image-rs` rejects with `Unknown bitmap header type (size=64)`.
pub fn decode_bmp_tolerant(bytes: &[u8]) -> Option<DynamicImage> {
    if let Ok(img) = decode_image_with_limits(bytes, Some(image::ImageFormat::Bmp)) {
        return Some(img);
    }
    if !bytes.starts_with(b"BM") || bytes.len() < 30 {
        return None;
    }
    let dib_size = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);

    if dib_size == 64 && bytes.len() >= 78 {
        // OS/2 v2 BITMAPINFOHEADER2: first 40 bytes match Windows BITMAPINFOHEADER.
        // Strip the 24 trailing OS/2-specific bytes (54..78) and adjust bfSize & bfOffBits by -24.
        let orig_off = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
        let new_off = orig_off.saturating_sub(24);
        let new_len = (bytes.len() - 24) as u32;

        let mut norm = Vec::with_capacity(bytes.len() - 24);
        norm.extend_from_slice(&bytes[0..2]); // "BM"
        norm.extend_from_slice(&new_len.to_le_bytes());
        norm.extend_from_slice(&bytes[6..10]); // reserved
        norm.extend_from_slice(&new_off.to_le_bytes());
        norm.extend_from_slice(&40u32.to_le_bytes()); // biSize = 40
        norm.extend_from_slice(&bytes[18..54]); // remaining 36 bytes of BITMAPINFOHEADER
        norm.extend_from_slice(&bytes[78..]); // palette + pixel data
        if let Ok(img) = decode_image_with_limits(&norm, Some(image::ImageFormat::Bmp)) {
            return Some(img);
        }
    } else if dib_size == 16 && bytes.len() >= 30 {
        // Short OS/2 v2 header (16 bytes): zero-pad to 40 bytes and adjust bfOffBits by +24.
        let orig_off = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
        let new_off = orig_off.saturating_add(24);
        let new_len = (bytes.len() + 24) as u32;

        let mut norm = Vec::with_capacity(bytes.len() + 24);
        norm.extend_from_slice(&bytes[0..2]);
        norm.extend_from_slice(&new_len.to_le_bytes());
        norm.extend_from_slice(&bytes[6..10]);
        norm.extend_from_slice(&new_off.to_le_bytes());
        norm.extend_from_slice(&40u32.to_le_bytes());
        norm.extend_from_slice(&bytes[18..30]);
        norm.extend_from_slice(&[0u8; 24]);
        norm.extend_from_slice(&bytes[30..]);
        if let Ok(img) = decode_image_with_limits(&norm, Some(image::ImageFormat::Bmp)) {
            return Some(img);
        }
    }

    None
}

/// Pure-Rust tolerant single-frame GIF LZW decoder for files truncated mid-sub-block
/// where most pixels are still intact before premature EOF.
fn decode_truncated_gif(bytes: &[u8]) -> Option<DynamicImage> {
    if bytes.len() < 24 || (!bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a")) {
        return None;
    }
    let screen_w = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let screen_h = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    if screen_w == 0 || screen_h == 0 || screen_w > 16384 || screen_h > 16384 {
        return None;
    }
    let packed = bytes[10];
    let mut pos = 13usize;

    let gct: Vec<[u8; 3]> = if (packed & 0x80) != 0 {
        let count = 1usize << ((packed & 0x07) + 1);
        if pos + count * 3 > bytes.len() {
            return None;
        }
        let mut pal = Vec::with_capacity(count);
        for i in 0..count {
            let o = pos + i * 3;
            pal.push([bytes[o], bytes[o + 1], bytes[o + 2]]);
        }
        pos += count * 3;
        pal
    } else {
        Vec::new()
    };

    let mut transp_idx: Option<u8> = None;

    // Walk extensions until the first Image Descriptor (0x2C)
    while pos < bytes.len() {
        match bytes[pos] {
            0x21 => {
                // Extension block
                if pos + 2 >= bytes.len() {
                    return None;
                }
                let label = bytes[pos + 1];
                pos += 2;
                if label == 0xF9 && pos + 5 <= bytes.len() && bytes[pos] == 4 {
                    let gce_flags = bytes[pos + 1];
                    if (gce_flags & 0x01) != 0 {
                        transp_idx = Some(bytes[pos + 4]);
                    }
                }
                while pos < bytes.len() {
                    let sub_len = bytes[pos] as usize;
                    pos += 1;
                    if sub_len == 0 {
                        break;
                    }
                    pos = pos.saturating_add(sub_len).min(bytes.len());
                }
            }
            0x2C => {
                // Image Descriptor
                if pos + 11 > bytes.len() {
                    return None;
                }
                let img_left = u16::from_le_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
                let img_top = u16::from_le_bytes([bytes[pos + 3], bytes[pos + 4]]) as usize;
                let img_w = u16::from_le_bytes([bytes[pos + 5], bytes[pos + 6]]) as usize;
                let img_h = u16::from_le_bytes([bytes[pos + 7], bytes[pos + 8]]) as usize;
                let img_packed = bytes[pos + 9];
                pos += 10;

                if img_w == 0 || img_h == 0 || img_w > 16384 || img_h > 16384 {
                    return None;
                }
                let interlaced = (img_packed & 0x40) != 0;
                let lct: Vec<[u8; 3]> = if (img_packed & 0x80) != 0 {
                    let count = 1usize << ((img_packed & 0x07) + 1);
                    if pos + count * 3 > bytes.len() {
                        return None;
                    }
                    let mut pal = Vec::with_capacity(count);
                    for i in 0..count {
                        let o = pos + i * 3;
                        pal.push([bytes[o], bytes[o + 1], bytes[o + 2]]);
                    }
                    pos += count * 3;
                    pal
                } else {
                    Vec::new()
                };
                let palette = if !lct.is_empty() { &lct } else { &gct };
                if palette.is_empty() || pos >= bytes.len() {
                    return None;
                }

                let min_code_size = bytes[pos] as u32;
                pos += 1;
                if !(2..=11).contains(&min_code_size) {
                    return None;
                }

                // Gather all sub-block bytes, gracefully clamping a truncated final sub-block
                let mut lzw_data = Vec::new();
                while pos < bytes.len() {
                    let sub_len = bytes[pos] as usize;
                    pos += 1;
                    if sub_len == 0 {
                        break;
                    }
                    let take = sub_len.min(bytes.len() - pos);
                    lzw_data.extend_from_slice(&bytes[pos..pos + take]);
                    pos += take;
                    if take < sub_len {
                        break;
                    }
                }
                if lzw_data.is_empty() {
                    return None;
                }

                // Decode GIF variable-width LSB-first LZW stream
                let expected_px = img_w.saturating_mul(img_h);
                let indices = decode_gif_lzw(&lzw_data, min_code_size, expected_px);
                if indices.is_empty() {
                    return None;
                }

                let canvas_w = screen_w.max(img_left + img_w);
                let canvas_h = screen_h.max(img_top + img_h);
                let mut rgba = vec![0u8; canvas_w * canvas_h * 4];

                // Build interlace row mapping if needed
                let mut row_map = Vec::with_capacity(img_h);
                if interlaced {
                    for &(start, step) in &[(0usize, 8usize), (4, 8), (2, 4), (1, 2)] {
                        let mut y = start;
                        while y < img_h {
                            row_map.push(y);
                            y += step;
                        }
                    }
                }

                for (i, &idx) in indices.iter().enumerate() {
                    let src_y = i / img_w;
                    if src_y >= img_h {
                        break;
                    }
                    let y = img_top + if interlaced { row_map[src_y] } else { src_y };
                    let x = img_left + (i % img_w);
                    if x < canvas_w && y < canvas_h {
                        let o = (y * canvas_w + x) * 4;
                        if transp_idx == Some(idx) {
                            rgba[o..o + 4].copy_from_slice(&[0, 0, 0, 0]);
                        } else if let Some(&rgb) = palette.get(idx as usize) {
                            rgba[o] = rgb[0];
                            rgba[o + 1] = rgb[1];
                            rgba[o + 2] = rgb[2];
                            rgba[o + 3] = 255;
                        }
                    }
                }

                let buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(
                    canvas_w as u32,
                    canvas_h as u32,
                    rgba,
                )?;
                return Some(DynamicImage::ImageRgba8(buf));
            }
            0x3B => break,
            _ => pos += 1,
        }
    }
    None
}

fn decode_gif_lzw(data: &[u8], min_code_size: u32, max_pixels: usize) -> Vec<u8> {
    let clear_code = 1u16 << min_code_size;
    let eoi_code = clear_code + 1;

    let mut prefix = [0u16; 4096];
    let mut suffix = [0u8; 4096];
    let mut stack = [0u8; 4096];

    for i in 0..clear_code as usize {
        suffix[i] = i as u8;
    }

    let mut code_size = min_code_size + 1;
    let mut next_code = eoi_code + 1;
    let mut prev_code: Option<u16> = None;
    let mut first_char = 0u8;

    let mut out = Vec::with_capacity(max_pixels.min(1_000_000));
    let mut bit_buf = 0u32;
    let mut bits_in_buf = 0u32;
    let mut byte_pos = 0usize;

    loop {
        while bits_in_buf < code_size {
            if byte_pos >= data.len() {
                return out;
            }
            bit_buf |= (data[byte_pos] as u32) << bits_in_buf;
            bits_in_buf += 8;
            byte_pos += 1;
        }
        let code = (bit_buf & ((1u32 << code_size) - 1)) as u16;
        bit_buf >>= code_size;
        bits_in_buf -= code_size;

        if code == clear_code {
            code_size = min_code_size + 1;
            next_code = eoi_code + 1;
            prev_code = None;
            continue;
        }
        if code == eoi_code {
            break;
        }

        let mut sp = 0usize;
        let mut cur = code;
        if cur >= next_code {
            if cur == next_code {
                if let Some(prev) = prev_code {
                    stack[sp] = first_char;
                    sp += 1;
                    cur = prev;
                } else {
                    break;
                }
            } else {
                // Corrupt code — return pixels recovered so far
                break;
            }
        }

        let mut steps = 0usize;
        while cur >= clear_code && sp < 4096 && steps < 4096 {
            stack[sp] = suffix[cur as usize];
            sp += 1;
            cur = prefix[cur as usize];
            steps += 1;
        }
        if sp >= 4096 || steps >= 4096 {
            break;
        }
        first_char = suffix[cur as usize];
        stack[sp] = first_char;
        sp += 1;

        while sp > 0 {
            sp -= 1;
            out.push(stack[sp]);
            if out.len() >= max_pixels {
                return out;
            }
        }

        if let Some(prev) = prev_code {
            if (next_code as usize) < 4096 {
                prefix[next_code as usize] = prev;
                suffix[next_code as usize] = first_char;
                next_code += 1;
                if next_code == (1u16 << code_size) && code_size < 12 {
                    code_size += 1;
                }
            }
        }
        prev_code = Some(code);
    }
    out
}

/// Partial-scanline recovery for PNG files whose `IDAT` deflate stream was truncated prematurely.
fn decode_truncated_png(bytes: &[u8]) -> Option<DynamicImage> {
    if bytes.len() < 33 || !bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return None;
    }

    let mut pos = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat = Vec::new();

    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
        let ctype = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start.saturating_add(len).min(bytes.len());
        let chunk = &bytes[data_start..data_end];

        match ctype {
            b"IHDR" if chunk.len() >= 13 => {
                width = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                height = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                bit_depth = chunk[8];
                color_type = chunk[9];
                interlace = chunk[12];
            }
            b"PLTE" => {
                for rgb in chunk.chunks_exact(3) {
                    palette.push([rgb[0], rgb[1], rgb[2]]);
                }
            }
            b"tRNS" => {
                trns.extend_from_slice(chunk);
            }
            b"IDAT" => {
                idat.extend_from_slice(chunk);
            }
            b"IEND" => break,
            _ => {}
        }
        pos = data_end.saturating_add(4);
    }

    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || interlace != 0
        || bit_depth != 8
        || idat.is_empty()
    {
        return None;
    }

    let bpp: usize = match color_type {
        0 => 1, // Grayscale
        2 => 3, // RGB
        3 => 1, // Indexed
        4 => 2, // GrayAlpha
        6 => 4, // RGBA
        _ => return None,
    };
    let stride = (width as usize).checked_mul(bpp)?;
    let row_bytes = stride + 1;
    let expected_raw = row_bytes.checked_mul(height as usize)?;
    if expected_raw > 256 * 1024 * 1024 {
        return None;
    }

    // Decompress as many bytes as are available in the truncated zlib stream
    let mut dec = flate2::read::ZlibDecoder::new(&idat[..]);
    let mut raw = Vec::new();
    let mut tmp = [0u8; 8192];
    while raw.len() < expected_raw {
        match dec.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                let take = n.min(expected_raw - raw.len());
                raw.extend_from_slice(&tmp[..take]);
            }
            Err(_) => break,
        }
    }

    let complete_rows = raw.len() / row_bytes;
    if complete_rows == 0 {
        return None;
    }

    let mut prev_row = vec![0u8; stride];
    let mut cur_row = vec![0u8; stride];
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];

    for y in 0..complete_rows {
        let src = &raw[y * row_bytes..(y + 1) * row_bytes];
        let filter = src[0];
        let data = &src[1..];
        for i in 0..stride {
            let a = if i >= bpp { cur_row[i - bpp] } else { 0 };
            let b = prev_row[i];
            let c = if i >= bpp { prev_row[i - bpp] } else { 0 };
            let recon = match filter {
                0 => data[i],
                1 => data[i].wrapping_add(a),
                2 => data[i].wrapping_add(b),
                3 => data[i].wrapping_add((((a as u16) + (b as u16)) / 2) as u8),
                4 => {
                    let p = a as i32 + b as i32 - c as i32;
                    let pa = (p - a as i32).abs();
                    let pb = (p - b as i32).abs();
                    let pc = (p - c as i32).abs();
                    let pr = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    data[i].wrapping_add(pr)
                }
                _ => data[i],
            };
            cur_row[i] = recon;
        }

        let dst = &mut rgba[y * (width as usize) * 4..(y + 1) * (width as usize) * 4];
        for x in 0..(width as usize) {
            let o = x * 4;
            match color_type {
                0 => {
                    let g = cur_row[x];
                    dst[o..o + 4].copy_from_slice(&[g, g, g, 255]);
                }
                2 => {
                    let s = x * 3;
                    dst[o..o + 4].copy_from_slice(&[cur_row[s], cur_row[s + 1], cur_row[s + 2], 255]);
                }
                3 => {
                    let idx = cur_row[x] as usize;
                    let rgb = palette.get(idx).copied().unwrap_or([0, 0, 0]);
                    let a = trns.get(idx).copied().unwrap_or(255);
                    dst[o..o + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
                }
                4 => {
                    let s = x * 2;
                    let g = cur_row[s];
                    dst[o..o + 4].copy_from_slice(&[g, g, g, cur_row[s + 1]]);
                }
                6 => {
                    let s = x * 4;
                    dst[o..o + 4].copy_from_slice(&cur_row[s..s + 4]);
                }
                _ => {}
            }
        }
        std::mem::swap(&mut prev_row, &mut cur_row);
    }

    let buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba)?;
    Some(DynamicImage::ImageRgba8(buf))
}

/// Attempts to recover truncated JPEG files by stripping malformed trailing APP/COM metadata
/// segments or appending missing zero-padding + EOI (`FF D9`) markers.
fn decode_truncated_jpeg(bytes: &[u8]) -> Option<DynamicImage> {
    if bytes.len() < 16 || !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }

    // Attempt 1: strip any truncated APP/COM segments before SOS and ensure trailing FF D9
    let mut cleaned = Vec::with_capacity(bytes.len() + 64);
    cleaned.extend_from_slice(&[0xFF, 0xD8]);
    let mut pos = 2usize;
    let mut saw_sos = false;

    while pos + 2 <= bytes.len() {
        if bytes[pos] != 0xFF {
            pos += 1;
            continue;
        }
        while pos < bytes.len() && bytes[pos] == 0xFF {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let marker = bytes[pos];
        pos += 1;

        if marker == 0xD9 {
            cleaned.extend_from_slice(&[0xFF, 0xD9]);
            break;
        }
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            cleaned.extend_from_slice(&[0xFF, marker]);
            continue;
        }
        if pos + 2 > bytes.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        if seg_len < 2 {
            break;
        }

        if marker == 0xDA {
            // SOS: copy header + entire remaining entropy scan data
            saw_sos = true;
            cleaned.extend_from_slice(&[0xFF, 0xDA]);
            cleaned.extend_from_slice(&bytes[pos..]);
            break;
        }

        if pos + seg_len > bytes.len() {
            // Truncated segment before SOS — drop APP/COM metadata if truncated; stop otherwise
            if (0xE0..=0xEF).contains(&marker) || marker == 0xFE {
                break;
            }
            break;
        }

        cleaned.extend_from_slice(&[0xFF, marker]);
        cleaned.extend_from_slice(&bytes[pos..pos + seg_len]);
        pos += seg_len;
    }

    if saw_sos {
        if !cleaned.ends_with(&[0xFF, 0xD9]) {
            cleaned.extend_from_slice(&[0xFF, 0xD9]);
        }
        if let Ok(img) = decode_image_with_limits(&cleaned, Some(image::ImageFormat::Jpeg)) {
            return Some(img);
        }
        // Pad scan stream with zero bytes before EOI in case final MCU bits were truncated
        if cleaned.ends_with(&[0xFF, 0xD9]) {
            cleaned.truncate(cleaned.len() - 2);
        }
        cleaned.extend_from_slice(&[0u8; 4096]);
        cleaned.extend_from_slice(&[0xFF, 0xD9]);
        if let Ok(img) = decode_image_with_limits(&cleaned, Some(image::ImageFormat::Jpeg)) {
            return Some(img);
        }
    }

    None
}

fn extract_exif_metadata(
    bytes: &[u8],
    keywords: &mut Vec<String>,
    metadata: &mut BTreeMap<String, String>,
) {
    let mut cursor = std::io::Cursor::new(bytes);
    let Ok(exif_data) = exif::Reader::new().read_from_container(&mut cursor) else {
        return;
    };
    // Read Windows Explorer EXIF XPKeywords (tag 0x9C9E, UTF-16LE semicolon-separated)
    let xp_keywords_tag = exif::Tag(exif::Context::Tiff, 0x9c9e);
    if let Some(field) = exif_data.get_field(xp_keywords_tag, exif::In::PRIMARY) {
        if let exif::Value::Byte(ref raw) = field.value {
            let u16s: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&v| v != 0)
                .collect();
            let decoded = String::from_utf16_lossy(&u16s);
            if !decoded.trim().is_empty() {
                cdr::push_split_keywords(keywords, &decoded);
            }
        }
    }
    for (tag, label) in [
        (exif::Tag::Make, "Camera Make"),
        (exif::Tag::Model, "Camera Model"),
        (exif::Tag::DateTimeOriginal, "Date Taken"),
        (exif::Tag::ExposureTime, "Exposure"),
        (exif::Tag::FNumber, "Aperture"),
        (exif::Tag::PhotographicSensitivity, "ISO"),
        (exif::Tag::FocalLength, "Focal Length"),
        (exif::Tag::Software, "Software"),
        (exif::Tag::Artist, "Author"),
        (exif::Tag::ImageDescription, "Description"),
    ] {
        if let Some(field) = exif_data.get_field(tag, exif::In::PRIMARY) {
            let val = field.display_value().with_unit(&exif_data).to_string();
            let clean = val.trim().trim_matches('"').to_string();
            if !clean.is_empty() {
                metadata.entry(label.into()).or_insert(clean);
            }
        }
    }
}

fn error_media(filetype: &str, msg: String) -> DecodedMedia {
    let mut metadata = BTreeMap::new();
    metadata.insert("Error".into(), msg.clone());
    DecodedMedia {
        image: None,
        filetype: filetype.to_string(),
        width: 0,
        height: 0,
        color_depth: 0,
        page_count: 1,
        keywords: Vec::new(),
        metadata,
        md5: None,
        source_note: "Error".into(),
        log_line: msg,
    }
}

/// Applies 3x3 convolution and point image filters for live preview inspection.
pub fn apply_filter(img: &DynamicImage, filter: ImageFilter) -> DynamicImage {
    if matches!(filter, ImageFilter::None) {
        return img.clone();
    }
    let src = img.to_rgba8();
    let (w, h) = src.dimensions();
    if matches!(filter, ImageFilter::Invert) {
        let mut out = src.clone();
        for p in out.pixels_mut() {
            p[0] = 255 - p[0];
            p[1] = 255 - p[1];
            p[2] = 255 - p[2];
        }
        return DynamicImage::ImageRgba8(out);
    }

    let (kernel, divisor, bias): ([i32; 9], i32, i32) = match filter {
        ImageFilter::Sharpen => ([-1, -1, -1, -1, 16, -1, -1, -1, -1], 8, 0),
        ImageFilter::Blur => ([1, 2, 1, 2, 4, 2, 1, 2, 1], 16, 0),
        ImageFilter::EmbossMedium => ([-1, -2, -1, 0, 0, 0, 1, 2, 1], 1, 192),
        ImageFilter::EdgesWeak => ([0, 1, 0, 1, -4, 1, 0, 1, 0], 1, 0),
        _ => return img.clone(),
    };

    let mut out = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0i32; 3];
            for ky in -1i32..=1 {
                for kx in -1i32..=1 {
                    let sx = (x as i32 + kx).clamp(0, w as i32 - 1) as u32;
                    let sy = (y as i32 + ky).clamp(0, h as i32 - 1) as u32;
                    let w_k = kernel[((ky + 1) * 3 + (kx + 1)) as usize];
                    let px = src.get_pixel(sx, sy);
                    acc[0] += px[0] as i32 * w_k;
                    acc[1] += px[1] as i32 * w_k;
                    acc[2] += px[2] as i32 * w_k;
                }
            }
            let a = src.get_pixel(x, y)[3];
            out.put_pixel(
                x,
                y,
                Rgba([
                    (acc[0] / divisor + bias).clamp(0, 255) as u8,
                    (acc[1] / divisor + bias).clamp(0, 255) as u8,
                    (acc[2] / divisor + bias).clamp(0, 255) as u8,
                    a,
                ]),
            );
        }
    }
    DynamicImage::ImageRgba8(out)
}

/// Decodes a JXL byte stream into a `DynamicImage` using pure-Rust `jxl-oxide`.
pub fn decode_jxl_bytes(bytes: &[u8]) -> Option<DynamicImage> {
    let jxl = jxl_oxide::JxlImage::builder()
        .read(std::io::Cursor::new(bytes))
        .ok()?;
    let w = jxl.width();
    let h = jxl.height();
    let render = jxl.render_frame(0).ok()?;
    let fb = render.image_all_channels();
    let channels = fb.channels();
    let samples = fb.buf();
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for px in samples.chunks_exact(channels) {
        let to_u8 = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
        match channels {
            1 => {
                let v = to_u8(px[0]);
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
            2 => {
                let v = to_u8(px[0]);
                rgba.extend_from_slice(&[v, v, v, to_u8(px[1])]);
            }
            3 => {
                rgba.extend_from_slice(&[to_u8(px[0]), to_u8(px[1]), to_u8(px[2]), 255]);
            }
            _ => {
                rgba.extend_from_slice(&[
                    to_u8(px[0]),
                    to_u8(px[1]),
                    to_u8(px[2]),
                    to_u8(px[3]),
                ]);
            }
        }
    }
    ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, rgba).map(DynamicImage::ImageRgba8)
}

fn image_has_transparency(rgba: &image::RgbaImage) -> bool {
    rgba.pixels().any(|p| p[3] < 255)
}

fn flatten_alpha_on_white(rgba: &image::RgbaImage) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((rgba.width() * rgba.height() * 3) as usize);
    for p in rgba.pixels() {
        let a = p[3] as u16;
        let inv = 255 - a;
        let r = ((p[0] as u16 * a + 255 * inv) / 255) as u8;
        let g = ((p[1] as u16 * a + 255 * inv) / 255) as u8;
        let b = ((p[2] as u16 * a + 255 * inv) / 255) as u8;
        rgb.extend_from_slice(&[r, g, b]);
    }
    rgb
}

fn encode_png_fast(rgba: &image::RgbaImage) -> Vec<u8> {
    let mut buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut buf,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    );
    let _ = image::ImageEncoder::write_image(
        encoder,
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        image::ExtendedColorType::Rgba8,
    );
    buf
}

/// Encodes a thumbnail in the configured format (WEBP, JXL, JPG, or PNG) at the given quality (1..=100).
/// Returns `(encoded_bytes, format_tag)`.
pub fn encode_thumbnail(
    img: &DynamicImage,
    format: ThumbFormat,
    quality: u8,
) -> (Vec<u8>, &'static str) {
    let q = quality.clamp(1, 100);
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return (Vec::new(), format.ext_tag());
    }
    let has_alpha = image_has_transparency(&rgba);

    match format {
        ThumbFormat::Webp => {
            let opts = if q >= 100 {
                oxideav_webp::EncodeOptions::default()
            } else {
                oxideav_webp::EncodeOptions::default().with_quality(q as f32)
            };
            let res = if has_alpha {
                oxideav_webp::encode_rgba8(w, h, rgba.as_raw(), &opts)
            } else {
                let rgb = img.to_rgb8();
                oxideav_webp::encode_rgb8(w, h, rgb.as_raw(), &opts)
            };
            if let Ok(bytes) = res {
                if !bytes.is_empty() {
                    return (bytes, "webp");
                }
            }
            (encode_png_fast(&rgba), "png")
        }
        ThumbFormat::Jxl => {
            let res = if q >= 100 {
                if has_alpha {
                    jxl_encoder::LosslessConfig::new().encode(
                        rgba.as_raw(),
                        w,
                        h,
                        jxl_encoder::PixelLayout::Rgba8,
                    )
                } else {
                    let rgb = img.to_rgb8();
                    jxl_encoder::LosslessConfig::new().encode(
                        rgb.as_raw(),
                        w,
                        h,
                        jxl_encoder::PixelLayout::Rgb8,
                    )
                }
            } else {
                let dist = jxl_encoder::quality_to_distance(q as f32);
                if has_alpha {
                    jxl_encoder::LossyConfig::new(dist).encode(
                        rgba.as_raw(),
                        w,
                        h,
                        jxl_encoder::PixelLayout::Rgba8,
                    )
                } else {
                    let rgb = img.to_rgb8();
                    jxl_encoder::LossyConfig::new(dist).encode(
                        rgb.as_raw(),
                        w,
                        h,
                        jxl_encoder::PixelLayout::Rgb8,
                    )
                }
            };
            if let Ok(bytes) = res {
                if !bytes.is_empty() {
                    return (bytes, "jxl");
                }
            }
            (encode_png_fast(&rgba), "png")
        }
        ThumbFormat::Jpg => {
            let rgb = if has_alpha {
                flatten_alpha_on_white(&rgba)
            } else {
                img.to_rgb8().into_raw()
            };
            let mut buf = Vec::new();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
            if image::ImageEncoder::write_image(
                encoder,
                &rgb,
                w,
                h,
                image::ExtendedColorType::Rgb8,
            )
            .is_ok()
                && !buf.is_empty()
            {
                return (buf, "jpg");
            }
            (encode_png_fast(&rgba), "png")
        }
        ThumbFormat::Png => (encode_png_fast(&rgba), "png"),
    }
}

/// Decodes a stored thumbnail blob from `thumbcraft.tcdb` (WEBP, JXL, JPG, or PNG).
pub fn decode_thumbnail_blob(bytes: &[u8], fmt: &str) -> Option<DynamicImage> {
    if bytes.is_empty() {
        return None;
    }
    let low = fmt.to_ascii_lowercase();
    let is_jxl = low == "jxl"
        || bytes.starts_with(&[0xFF, 0x0A])
        || bytes.starts_with(&[0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ']);
    if is_jxl {
        return decode_jxl_bytes(bytes);
    }

    let is_webp = low == "webp"
        || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP");
    if is_webp {
        if let Ok(img) = oxideav_webp::decode_rgba8(bytes) {
            if let Some(buf) = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(img.width, img.height, img.data) {
                return Some(DynamicImage::ImageRgba8(buf));
            }
        }
    }

    image::load_from_memory(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image() -> DynamicImage {
        let mut buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(64, 48);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            *p = Rgba([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8, 255]);
        }
        DynamicImage::ImageRgba8(buf)
    }

    #[test]
    fn thumbnail_codec_roundtrip_all_formats() {
        let img = test_image();
        for (fmt, expected_tag) in [
            (ThumbFormat::Webp, "webp"),
            (ThumbFormat::Jxl, "jxl"),
            (ThumbFormat::Jpg, "jpg"),
            (ThumbFormat::Png, "png"),
        ] {
            let (blob, tag) = encode_thumbnail(&img, fmt, 80);
            assert_eq!(tag, expected_tag, "format tag mismatch for {fmt:?}");
            assert!(!blob.is_empty(), "empty encoded blob for {fmt:?}");
            let decoded = decode_thumbnail_blob(&blob, tag)
                .unwrap_or_else(|| panic!("failed to decode {fmt:?} blob"));
            assert_eq!(decoded.width(), 64);
            assert_eq!(decoded.height(), 48);
        }
    }

    #[test]
    fn supported_extensions_and_gif_multi_frame_decode() {
        assert!(is_supported_file(Path::new("art.psd")));
        assert!(is_supported_file(Path::new("huge.psb")));
        assert!(is_supported_file(Path::new("anim.gif")));

        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_gif_test_{}.gif",
            std::process::id()
        ));
        {
            let file = std::fs::File::create(&tmp).unwrap();
            let mut enc = image::codecs::gif::GifEncoder::new(file);
            let f1 = image::Frame::new(ImageBuffer::from_pixel(16, 12, Rgba([255, 0, 0, 255])));
            let f2 = image::Frame::new(ImageBuffer::from_pixel(16, 12, Rgba([0, 0, 255, 255])));
            enc.encode_frames([f1, f2]).unwrap();
        }

        let gs = GhostscriptConfig::default();
        let dec1 = decode_preview_page(&tmp, &gs, 1, false);
        assert_eq!(dec1.filetype, "GIF");
        assert_eq!(dec1.width, 16);
        assert_eq!(dec1.height, 12);
        assert_eq!(dec1.page_count, 2);
        assert!(dec1.image.is_some());

        let dec2 = decode_preview_page(&tmp, &gs, 2, false);
        let px2 = dec2.image.unwrap().to_rgba8().get_pixel(0, 0).0;
        assert!(px2[2] > 200 && px2[0] < 50, "expected blue frame 2, got {px2:?}");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn decodes_misnamed_webp_as_jpeg_and_truncated_gif() {
        let gs = GhostscriptConfig {
            gs_path: String::new(),
            ..Default::default()
        };
        let img = test_image();

        // 1. WebP bytes saved with a .jpg extension must decode via magic-byte sniffing
        let (webp_bytes, _) = encode_thumbnail(&img, ThumbFormat::Webp, 80);
        let tmp_jpg = std::env::temp_dir().join(format!(
            "thumbcraft_misnamed_webp_{}.jpg",
            std::process::id()
        ));
        std::fs::write(&tmp_jpg, &webp_bytes).unwrap();
        let dec_webp = decode_thumbnail(&tmp_jpg, &gs);
        let _ = std::fs::remove_file(&tmp_jpg);
        assert!(dec_webp.image.is_some(), "misnamed WebP should decode cleanly");
        assert_eq!(dec_webp.width, 64);
        assert_eq!(dec_webp.height, 48);

        // 2. Truncated GIF (missing final LZW sub-block bytes and 0x3B trailer) must still recover
        let mut gif_buf = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut gif_buf);
            let frame = image::Frame::new(img.to_rgba8());
            enc.encode_frame(frame).unwrap();
        }
        let trunc_gif = &gif_buf[..gif_buf.len().saturating_sub(12)];
        let recovered = decode_truncated_gif(trunc_gif)
            .expect("truncated GIF should recover partial frame");
        assert_eq!(recovered.width(), 64);
        assert_eq!(recovered.height(), 48);
    }

    #[test]
    fn decodes_os2_v2_64byte_bmp_png_as_webp_and_truncated_png() {
        // 1. Synthetic OS/2 v2 64-byte header 2x2 24-bit BMP
        let mut bmp = Vec::new();
        let pixel_off = 14u32 + 64;
        let row_stride = 8u32; // 2 pixels * 3 bytes = 6, padded to 8
        let file_sz = pixel_off + row_stride * 2;
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&file_sz.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&pixel_off.to_le_bytes());
        bmp.extend_from_slice(&64u32.to_le_bytes()); // OS/2 v2 BITMAPINFOHEADER2 size=64
        bmp.extend_from_slice(&2i32.to_le_bytes()); // width
        bmp.extend_from_slice(&2i32.to_le_bytes()); // height
        bmp.extend_from_slice(&1u16.to_le_bytes()); // planes
        bmp.extend_from_slice(&24u16.to_le_bytes()); // bpp
        bmp.extend_from_slice(&[0u8; 24]); // rest of first 40 bytes
        bmp.extend_from_slice(&[0u8; 24]); // extra 24 OS/2 v2 bytes
        // Bottom row then top row (BGR + 2 pad bytes)
        bmp.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        bmp.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);
        let img = decode_bmp_tolerant(&bmp).expect("OS/2 v2 64-byte BMP must decode");
        assert_eq!(img.width(), 2);
        assert_eq!(img.height(), 2);

        // 2. PNG bytes truncated mid-IDAT should still recover partial scanlines
        let full_png = encode_png_fast(&test_image().to_rgba8());
        let trunc = &full_png[..full_png.len() - 40];
        let rec = decode_truncated_png(trunc).expect("Truncated PNG should recover partial rows");
        assert_eq!(rec.width(), 64);
        assert_eq!(rec.height(), 48);

        // 3. PNG bytes saved with .webp extension should decode cleanly via magic bytes
        let (magic_img, kind) = decode_by_magic_bytes(&full_png).expect("PNG magic must decode");
        assert_eq!(kind, "PNG");
        assert_eq!(magic_img.width(), 64);
    }
}
