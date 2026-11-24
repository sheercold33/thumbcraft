use crate::config::GhostscriptConfig;
use crate::ghostscript;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub const KEY_SUBSAMPLING: &str = "Subsampling";
pub const KEY_ESTIMATED_QUALITY: &str = "Estimated Quality";
pub const KEY_BITS_PER_CHANNEL: &str = "Bits Per Channel";
pub const KEY_DPI: &str = "DPI";

#[derive(Debug, Default, Clone)]
pub struct TechnicalImageMeta {
    pub subsampling: Option<String>,
    pub estimated_quality: Option<String>,
    pub bits_per_channel: Option<String>,
    pub dpi: Option<String>,
}

impl TechnicalImageMeta {
    pub fn insert_into(&self, map: &mut BTreeMap<String, String>) {
        if let Some(ref v) = self.subsampling {
            map.insert(KEY_SUBSAMPLING.to_string(), v.clone());
        }
        if let Some(ref v) = self.estimated_quality {
            map.insert(KEY_ESTIMATED_QUALITY.to_string(), v.clone());
        }
        if let Some(ref v) = self.bits_per_channel {
            map.insert(KEY_BITS_PER_CHANNEL.to_string(), v.clone());
        }
        if let Some(ref v) = self.dpi {
            map.insert(KEY_DPI.to_string(), v.clone());
        }
    }
}

/// Fast header-only inspection for an on-disk image (reads up to 256 KB from the start of the
/// file so already-cataloged files populate technical fields immediately without full decoding).
pub fn inspect_file_technical_metadata(
    path: &Path,
    gs_config: Option<&GhostscriptConfig>,
) -> TechnicalImageMeta {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // Vector / Ghostscript formats don't need binary raster header scans
    match ext.as_str() {
        "pdf" | "ai" | "eps" | "epsi" | "epsf" | "ps" | "prn" => {
            let dpi = gs_config
                .map(|g| ghostscript::clamp_dpi(g.preview_dpi))
                .unwrap_or(150);
            let bpc = gs_config
                .map(|g| if g.color_depth == 1 { "1-bit" } else { "8-bit" })
                .unwrap_or("8-bit");
            return TechnicalImageMeta {
                subsampling: Some("None (Vector)".to_string()),
                estimated_quality: Some("Vector".to_string()),
                bits_per_channel: Some(bpc.to_string()),
                dpi: Some(format!("{dpi} DPI")),
            };
        }
        "svg" | "svgz" => {
            return TechnicalImageMeta {
                subsampling: Some("None (Vector)".to_string()),
                estimated_quality: Some("100% (Vector)".to_string()),
                bits_per_channel: Some("8-bit".to_string()),
                dpi: Some("96 DPI".to_string()),
            };
        }
        "cdr" | "cmx" | "ccx" | "pat" => {
            return TechnicalImageMeta {
                subsampling: Some("None (Vector)".to_string()),
                estimated_quality: Some("Vector".to_string()),
                bits_per_channel: Some("8-bit".to_string()),
                dpi: None,
            };
        }
        _ => {}
    }

    let Ok(mut f) = File::open(path) else {
        return TechnicalImageMeta::default();
    };
    let max_read = if matches!(ext.as_str(), "tif" | "tiff" | "psd" | "psb") {
        512 * 1024
    } else {
        256 * 1024
    };
    let mut buf = Vec::new();
    let _ = f.by_ref().take(max_read as u64).read_to_end(&mut buf);
    if buf.is_empty() {
        return TechnicalImageMeta::default();
    }
    inspect_bytes_technical_metadata(&buf, &ext)
}

/// Inspects raw image file bytes (header or complete file) for Subsampling, Estimated Quality,
/// Bits Per Channel, and DPI.
pub fn inspect_bytes_technical_metadata(bytes: &[u8], ext_lower: &str) -> TechnicalImageMeta {
    if bytes.starts_with(&[0xFF, 0xD8]) || matches!(ext_lower, "jpg" | "jpeg" | "jpe") {
        return inspect_jpeg(bytes);
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) || ext_lower == "png" {
        return inspect_png(bytes);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return inspect_webp(bytes);
    }
    if bytes.starts_with(b"BM") || matches!(ext_lower, "bmp" | "dib") {
        return inspect_bmp(bytes);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") || ext_lower == "gif" {
        return TechnicalImageMeta {
            subsampling: Some("None (Indexed)".to_string()),
            estimated_quality: Some("Lossless (Indexed)".to_string()),
            bits_per_channel: Some("8-bit".to_string()),
            dpi: None,
        };
    }
    if bytes.starts_with(b"8BPS") || matches!(ext_lower, "psd" | "psb") {
        return inspect_psd(bytes);
    }
    if (bytes.len() >= 4
        && (&bytes[0..4] == b"II*\0" || &bytes[0..4] == b"MM\0*"))
        || matches!(ext_lower, "tif" | "tiff")
    {
        return inspect_tiff(bytes);
    }
    if ext_lower == "avif" || (bytes.len() >= 12 && &bytes[4..8] == b"ftyp") {
        return inspect_avif(bytes);
    }
    if ext_lower == "jxl"
        || bytes.starts_with(&[0xFF, 0x0A])
        || bytes.starts_with(&[0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' '])
    {
        return inspect_jxl(bytes);
    }
    if crate::cpt::is_cpt_magic(bytes) || ext_lower == "cpt" {
        if let Some(info) = crate::cpt::inspect_cpt_header(bytes) {
            let dpi = match (info.dpi_x, info.dpi_y) {
                (Some(x), Some(y)) if x == y => Some(format!("{x} DPI")),
                (Some(x), Some(y)) => Some(format!("{x} × {y} DPI")),
                _ => None,
            };
            return TechnicalImageMeta {
                subsampling: Some("None (RGB)".to_string()),
                estimated_quality: Some("100% (Lossless)".to_string()),
                bits_per_channel: Some("8-bit".to_string()),
                dpi,
            };
        }
    }
    if bytes.starts_with(b"DDS ") || ext_lower == "dds" {
        if let Some(info) = crate::dds::inspect_dds_header(bytes) {
            return TechnicalImageMeta {
                subsampling: Some(if info.is_compressed {
                    format!("Block-Compressed ({})", info.format_label)
                } else {
                    "None (RGB)".to_string()
                }),
                estimated_quality: Some(if info.is_compressed {
                    format!("Block-Compressed ({})", info.format_label)
                } else {
                    "100% (Lossless)".to_string()
                }),
                bits_per_channel: Some(info.bits_per_channel),
                dpi: None,
            };
        }
    }
    if matches!(ext_lower, "qoi" | "tga" | "ico" | "cur" | "pnm" | "ppm" | "pgm" | "pbm") {
        return TechnicalImageMeta {
            subsampling: Some("None (RGB)".to_string()),
            estimated_quality: Some("100% (Lossless)".to_string()),
            bits_per_channel: Some("8-bit".to_string()),
            dpi: None,
        };
    }
    if matches!(ext_lower, "hdr" | "exr") {
        return TechnicalImageMeta {
            subsampling: Some("None (RGB)".to_string()),
            estimated_quality: Some("HDR".to_string()),
            bits_per_channel: Some(if ext_lower == "exr" {
                "16/32-bit (Float)".to_string()
            } else {
                "32-bit (Float)".to_string()
            }),
            dpi: None,
        };
    }
    TechnicalImageMeta::default()
}

pub fn bits_per_channel_from_color_type(ct: image::ColorType) -> &'static str {
    match ct {
        image::ColorType::L8
        | image::ColorType::La8
        | image::ColorType::Rgb8
        | image::ColorType::Rgba8 => "8-bit",
        image::ColorType::L16
        | image::ColorType::La16
        | image::ColorType::Rgb16
        | image::ColorType::Rgba16 => "16-bit",
        image::ColorType::Rgb32F | image::ColorType::Rgba32F => "32-bit (Float)",
        _ => "8-bit",
    }
}

fn format_dpi(x: f64, y: f64) -> Option<String> {
    if !x.is_finite() || !y.is_finite() || x < 1.0 || y < 1.0 || x > 50_000.0 || y > 50_000.0 {
        return None;
    }
    let rx = x.round() as u32;
    let ry = y.round() as u32;
    if rx == ry {
        Some(format!("{rx} DPI"))
    } else {
        Some(format!("{rx} × {ry} DPI"))
    }
}

fn extract_exif_dpi(bytes: &[u8]) -> Option<String> {
    let mut cursor = std::io::Cursor::new(bytes);
    let exif_data = exif::Reader::new().read_from_container(&mut cursor).ok()?;
    exif_dpi_from_parsed(&exif_data)
}

fn exif_dpi_from_parsed(exif_data: &exif::Exif) -> Option<String> {
    let x_rat = exif_data
        .get_field(exif::Tag::XResolution, exif::In::PRIMARY)
        .and_then(|f| match f.value {
            exif::Value::Rational(ref v) => v.first().map(|r| r.to_f64()),
            _ => None,
        })?;
    let y_rat = exif_data
        .get_field(exif::Tag::YResolution, exif::In::PRIMARY)
        .and_then(|f| match f.value {
            exif::Value::Rational(ref v) => v.first().map(|r| r.to_f64()),
            _ => None,
        })
        .unwrap_or(x_rat);
    let unit = exif_data
        .get_field(exif::Tag::ResolutionUnit, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or(2);
    let (dx, dy) = match unit {
        3 => (x_rat * 2.54, y_rat * 2.54), // pixels per cm -> DPI
        _ => (x_rat, y_rat),               // pixels per inch
    };
    format_dpi(dx, dy)
}

// ---------------------------------------------------------------------------
// JPEG: SOF subsampling & bit depth, JFIF/EXIF DPI, IJG DQT quality estimator
// ---------------------------------------------------------------------------

// Standard IJG Annex K quantization tables in JPEG zigzag scan order (exact DQT order).
const IJG_LUMA_ZIGZAG: [u32; 64] = [
    16, 11, 12, 14, 12, 10, 16, 14, 13, 14, 18, 17, 16, 19, 24, 40,
    26, 24, 22, 22, 24, 49, 35, 37, 29, 40, 58, 51, 61, 60, 57, 51,
    56, 55, 64, 72, 92, 78, 64, 68, 87, 69, 55, 56, 80, 109, 81, 87,
    95, 98, 103, 104, 103, 62, 77, 113, 121, 112, 100, 120, 92, 101, 103, 99,
];

const IJG_CHROMA_ZIGZAG: [u32; 64] = [
    17, 18, 18, 24, 21, 24, 47, 26, 26, 47, 99, 66, 56, 66, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

fn ijg_scaled_q(base: u32, quality: u32) -> u32 {
    let scale = if quality < 50 {
        5000 / quality.max(1)
    } else {
        200 - 2 * quality.min(100)
    };
    ((base * scale + 50) / 100).clamp(1, 255)
}

fn estimate_jpeg_quality(luma: Option<&[u16; 64]>, chroma: Option<&[u16; 64]>) -> Option<u32> {
    if luma.is_none() && chroma.is_none() {
        return None;
    }
    let mut best_q = 75u32;
    let mut best_err = u64::MAX;
    for q in 1u32..=100 {
        let mut err = 0u64;
        if let Some(lt) = luma {
            for i in 0..64 {
                let expected = ijg_scaled_q(IJG_LUMA_ZIGZAG[i], q);
                err += (lt[i] as u32).abs_diff(expected) as u64;
            }
        }
        if let Some(ct) = chroma {
            for i in 0..64 {
                let expected = ijg_scaled_q(IJG_CHROMA_ZIGZAG[i], q);
                err += (ct[i] as u32).abs_diff(expected) as u64;
            }
        }
        if err < best_err {
            best_err = err;
            best_q = q;
        }
    }
    Some(best_q)
}

fn inspect_jpeg(bytes: &[u8]) -> TechnicalImageMeta {
    let mut meta = TechnicalImageMeta::default();
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return meta;
    }

    let mut jfif_dpi: Option<String> = None;
    let mut luma_dqt: Option<[u16; 64]> = None;
    let mut chroma_dqt: Option<[u16; 64]> = None;
    let mut is_lossless = false;

    let mut pos = 2usize;
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

        // Standalone markers or SOS (image entropy data starts here) / EOI
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        if marker == 0x00 || (0xD0..=0xD8).contains(&marker) || marker == 0x01 {
            continue;
        }
        if pos + 2 > bytes.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        if seg_len < 2 || pos + seg_len > bytes.len() {
            break;
        }
        let seg = &bytes[pos + 2..pos + seg_len];
        pos += seg_len;

        match marker {
            // APP0 JFIF density
            0xE0 if seg.len() >= 14 && &seg[0..5] == b"JFIF\0" => {
                let units = seg[7];
                let xd = u16::from_be_bytes([seg[8], seg[9]]) as f64;
                let yd = u16::from_be_bytes([seg[10], seg[11]]) as f64;
                match units {
                    1 => jfif_dpi = format_dpi(xd, yd),
                    2 => jfif_dpi = format_dpi(xd * 2.54, yd * 2.54),
                    _ => {}
                }
            }
            // DQT (Define Quantization Table)
            0xDB => {
                let mut qpos = 0usize;
                while qpos < seg.len() {
                    let info = seg[qpos];
                    qpos += 1;
                    let precision = info >> 4;
                    let table_id = info & 0x0F;
                    let elem_size = if precision == 0 { 1 } else { 2 };
                    if qpos + 64 * elem_size > seg.len() {
                        break;
                    }
                    let mut table = [0u16; 64];
                    if precision == 0 {
                        for i in 0..64 {
                            table[i] = seg[qpos + i] as u16;
                        }
                    } else {
                        for i in 0..64 {
                            table[i] =
                                u16::from_be_bytes([seg[qpos + i * 2], seg[qpos + i * 2 + 1]]);
                        }
                    }
                    qpos += 64 * elem_size;
                    if table_id == 0 {
                        luma_dqt = Some(table);
                    } else if table_id == 1 {
                        chroma_dqt = Some(table);
                    }
                }
            }
            // SOF markers (Baseline, Extended, Progressive, Lossless, etc.)
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                if matches!(marker, 0xC3 | 0xC7 | 0xCB | 0xCF) {
                    is_lossless = true;
                }
                if seg.len() >= 6 {
                    let precision = seg[0];
                    meta.bits_per_channel = Some(format!("{precision}-bit"));
                    let nf = seg[5] as usize;
                    if nf == 1 {
                        meta.subsampling = Some("4:0:0 (Grayscale)".to_string());
                    } else if nf >= 3 && seg.len() >= 6 + nf * 3 {
                        let hv1 = seg[7];
                        let hv2 = seg[10];
                        let hv3 = seg[13];
                        let (h1, v1) = (hv1 >> 4, hv1 & 0x0F);
                        let (h2, v2) = (hv2 >> 4, hv2 & 0x0F);
                        let (h3, v3) = (hv3 >> 4, hv3 & 0x0F);
                        let sub = if h2 == h3 && v2 == v3 && h2 > 0 && v2 > 0 {
                            match (h1 / h2, v1 / v2) {
                                (1, 1) => "4:4:4".to_string(),
                                (2, 1) => "4:2:2".to_string(),
                                (2, 2) => "4:2:0".to_string(),
                                (1, 2) => "4:4:0".to_string(),
                                (4, 1) => "4:1:1".to_string(),
                                (4, 2) => "4:1:0".to_string(),
                                _ => format!("{h1}×{v1}, {h2}×{v2}, {h3}×{v3}"),
                            }
                        } else {
                            format!("{h1}×{v1}, {h2}×{v2}, {h3}×{v3}")
                        };
                        meta.subsampling = Some(sub);
                    }
                }
            }
            _ => {}
        }
    }

    if is_lossless {
        meta.estimated_quality = Some("100% (Lossless)".to_string());
    } else if let Some(q) = estimate_jpeg_quality(luma_dqt.as_ref(), chroma_dqt.as_ref()) {
        meta.estimated_quality = Some(format!("{q}%"));
    }

    // Prefer EXIF DPI when present, fall back to JFIF APP0 density
    meta.dpi = extract_exif_dpi(bytes).or(jfif_dpi);
    meta
}

// ---------------------------------------------------------------------------
// PNG: IHDR bit depth & color type, pHYs chunk DPI, EXIF fallback
// ---------------------------------------------------------------------------

fn inspect_png(bytes: &[u8]) -> TechnicalImageMeta {
    let mut meta = TechnicalImageMeta {
        estimated_quality: Some("100% (Lossless)".to_string()),
        ..Default::default()
    };
    if bytes.len() < 33 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return meta;
    }

    let mut pos = 8usize;
    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let ctype = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start.saturating_add(len);
        if data_end > bytes.len() {
            break;
        }
        let data = &bytes[data_start..data_end];

        if ctype == b"IHDR" && data.len() >= 10 {
            let bit_depth = data[8];
            let color_type = data[9];
            meta.bits_per_channel = Some(format!("{bit_depth}-bit"));
            meta.subsampling = Some(
                match color_type {
                    0 => "None (Grayscale)",
                    2 => "None (RGB)",
                    3 => "None (Indexed)",
                    4 => "None (Gray+Alpha)",
                    6 => "None (RGBA)",
                    _ => "None",
                }
                .to_string(),
            );
        } else if ctype == b"pHYs" && data.len() >= 9 {
            let ppu_x = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as f64;
            let ppu_y = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as f64;
            let unit = data[8];
            if unit == 1 && ppu_x > 0.0 && ppu_y > 0.0 {
                meta.dpi = format_dpi(ppu_x * 0.0254, ppu_y * 0.0254);
            }
        } else if ctype == b"IDAT" || ctype == b"IEND" {
            break;
        }

        pos = data_end.saturating_add(4);
    }

    if meta.dpi.is_none() {
        meta.dpi = extract_exif_dpi(bytes);
    }
    meta
}

// ---------------------------------------------------------------------------
// WEBP: VP8 lossy (4:2:0 + VP8 boolean header base_q_index) vs VP8L lossless
// ---------------------------------------------------------------------------

struct Vp8BoolReader<'a> {
    data: &'a [u8],
    pos: usize,
    value: u32,
    range: u32,
    bit_count: i32,
}

impl<'a> Vp8BoolReader<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() < 2 {
            return None;
        }
        let value = ((data[0] as u32) << 8) | (data[1] as u32);
        Some(Self {
            data,
            pos: 2,
            value,
            range: 255,
            bit_count: 0,
        })
    }

    fn read_bool(&mut self, prob: u32) -> Option<u32> {
        let split = 1 + (((self.range - 1) * prob) >> 8);
        let split_shifted = split << 8;
        let bit = if self.value >= split_shifted {
            self.value -= split_shifted;
            self.range -= split;
            1
        } else {
            self.range = split;
            0
        };
        while self.range < 128 {
            self.range <<= 1;
            self.value <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.bit_count = 0;
                if self.pos >= self.data.len() {
                    return None;
                }
                self.value |= self.data[self.pos] as u32;
                self.pos += 1;
            }
        }
        Some(bit)
    }

    fn read_bit(&mut self) -> Option<u32> {
        self.read_bool(128)
    }

    fn read_uint(&mut self, bits: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..bits {
            v = (v << 1) | self.read_bit()?;
        }
        Some(v)
    }

    fn skip(&mut self, bits: u32) -> Option<()> {
        for _ in 0..bits {
            self.read_bit()?;
        }
        Some(())
    }
}

/// Parses a lossy VP8 keyframe's `base_q_index` (0..=127) and maps it to WebP quality (0..=100).
fn estimate_vp8_quality(vp8: &[u8]) -> Option<u32> {
    if vp8.len() < 16 {
        return None;
    }
    // Uncompressed data chunk: 3-byte frame tag (bit 0 == 0 means keyframe) + 3-byte start code
    if (vp8[0] & 1) != 0 {
        return None;
    }
    if vp8[3] != 0x9D || vp8[4] != 0x01 || vp8[5] != 0x2A {
        return None;
    }
    let mut br = Vp8BoolReader::new(&vp8[10..])?;
    // color_space (1), clamping_type (1)
    br.skip(2)?;
    // segmentation_enabled (1)
    if br.read_bit()? != 0 {
        let update_map = br.read_bit()?;
        let update_data = br.read_bit()?;
        if update_data != 0 {
            let _abs_delta = br.read_bit()?;
            for _ in 0..4 {
                if br.read_bit()? != 0 {
                    br.skip(8)?; // 7-bit magnitude + 1-bit sign
                }
            }
            for _ in 0..4 {
                if br.read_bit()? != 0 {
                    br.skip(7)?; // 6-bit magnitude + 1-bit sign
                }
            }
        }
        if update_map != 0 {
            for _ in 0..3 {
                if br.read_bit()? != 0 {
                    br.skip(8)?;
                }
            }
        }
    }
    // filter_type (1), loop_filter_level (6), sharpness_level (3)
    br.skip(10)?;
    // mode_ref_lf_delta_enabled (1)
    if br.read_bit()? != 0 && br.read_bit()? != 0 {
        for _ in 0..4 {
            if br.read_bit()? != 0 {
                br.skip(7)?;
            }
        }
        for _ in 0..4 {
            if br.read_bit()? != 0 {
                br.skip(7)?;
            }
        }
    }
    // nbr_of_dct_partitions (2)
    br.skip(2)?;
    // y_ac_qi (7 bits): 0 = highest quality (100%), 127 = lowest quality (0%)
    let q_index = br.read_uint(7)?.min(127);

    // Invert libwebp's QualityToCompression curve:
    //   c = quality / 100.0
    //   q_index ≈ round(127.0 * (1.0 - c) ^ (1 / 0.85))  (libwebp uses a slight gamma curve)
    let linear = 1.0 - (q_index as f64 / 127.0);
    let est = (linear.powf(0.85) * 100.0).round().clamp(1.0, 100.0) as u32;
    Some(est)
}

fn inspect_webp(bytes: &[u8]) -> TechnicalImageMeta {
    let mut meta = TechnicalImageMeta {
        bits_per_channel: Some("8-bit".to_string()),
        ..Default::default()
    };
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return meta;
    }

    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let fourcc = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let data_start = pos + 8;
        let data_end = data_start.saturating_add(size).min(bytes.len());
        let payload = &bytes[data_start..data_end];

        if fourcc == b"VP8L" {
            meta.subsampling = Some("4:4:4 (Lossless)".to_string());
            meta.estimated_quality = Some("100% (Lossless)".to_string());
        } else if fourcc == b"VP8 " {
            meta.subsampling = Some("4:2:0".to_string());
            if let Some(q) = estimate_vp8_quality(payload) {
                meta.estimated_quality = Some(format!("{q}%"));
            }
        } else if fourcc == b"EXIF" && meta.dpi.is_none() {
            let exif_slice = payload.strip_prefix(b"Exif\0\0").unwrap_or(payload);
            if let Ok(parsed) = exif::Reader::new().read_raw(exif_slice.to_vec()) {
                meta.dpi = exif_dpi_from_parsed(&parsed);
            }
        }

        let padded = size.saturating_add(size & 1);
        let next = data_start.saturating_add(padded);
        if next <= pos {
            break;
        }
        pos = next;
    }

    if meta.dpi.is_none() {
        meta.dpi = extract_exif_dpi(bytes);
    }
    meta
}

// ---------------------------------------------------------------------------
// BMP: BITMAPINFOHEADER bit depth & pixels-per-meter DPI
// ---------------------------------------------------------------------------

fn inspect_bmp(bytes: &[u8]) -> TechnicalImageMeta {
    let mut meta = TechnicalImageMeta {
        estimated_quality: Some("100% (Lossless)".to_string()),
        ..Default::default()
    };
    if bytes.len() < 46 || &bytes[0..2] != b"BM" {
        return meta;
    }
    let dib_size = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    if dib_size >= 40 {
        let bpp = u16::from_le_bytes([bytes[28], bytes[29]]);
        let (bpc, sub) = match bpp {
            1 => ("1-bit", "None (Indexed)"),
            4 => ("4-bit", "None (Indexed)"),
            8 => ("8-bit", "None (Indexed)"),
            16 => ("5-bit", "None (RGB)"),
            24 => ("8-bit", "None (RGB)"),
            32 => ("8-bit", "None (RGBA)"),
            _ => ("8-bit", "None (RGB)"),
        };
        meta.bits_per_channel = Some(bpc.to_string());
        meta.subsampling = Some(sub.to_string());

        let x_ppm = i32::from_le_bytes([bytes[38], bytes[39], bytes[40], bytes[41]]);
        let y_ppm = i32::from_le_bytes([bytes[42], bytes[43], bytes[44], bytes[45]]);
        if x_ppm > 0 && y_ppm > 0 {
            meta.dpi = format_dpi(x_ppm as f64 * 0.0254, y_ppm as f64 * 0.0254);
        }
    }
    meta
}

// ---------------------------------------------------------------------------
// PSD / PSB: Header channel depth, color mode, Resource 1005 ResolutionInfo
// ---------------------------------------------------------------------------

fn inspect_psd(bytes: &[u8]) -> TechnicalImageMeta {
    let mut meta = TechnicalImageMeta {
        estimated_quality: Some("100% (Lossless)".to_string()),
        ..Default::default()
    };
    if bytes.len() < 34 || &bytes[0..4] != b"8BPS" {
        return meta;
    }
    let depth = u16::from_be_bytes([bytes[22], bytes[23]]);
    let color_mode = u16::from_be_bytes([bytes[24], bytes[25]]);
    meta.bits_per_channel = Some(if depth == 32 {
        "32-bit (Float)".to_string()
    } else {
        format!("{depth}-bit")
    });
    meta.subsampling = Some(
        match color_mode {
            0 => "None (Bitmap)",
            1 => "None (Grayscale)",
            2 => "None (Indexed)",
            3 => "None (RGB)",
            4 => "None (CMYK)",
            9 => "None (Lab)",
            _ => "None",
        }
        .to_string(),
    );

    let mut pos = 26usize;
    if pos + 4 > bytes.len() {
        return meta;
    }
    let cm_len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
        as usize;
    pos = pos.saturating_add(4 + cm_len);
    if pos + 4 > bytes.len() {
        return meta;
    }
    let res_len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
        as usize;
    pos += 4;
    let res_end = pos.saturating_add(res_len).min(bytes.len());
    let data = &bytes[pos..res_end];

    let mut rpos = 0usize;
    while rpos + 12 <= data.len() {
        if &data[rpos..rpos + 4] != b"8BIM" {
            break;
        }
        let res_id = u16::from_be_bytes([data[rpos + 4], data[rpos + 5]]);
        rpos += 6;
        if rpos >= data.len() {
            break;
        }
        let name_len = data[rpos] as usize;
        let padded_name = (1 + name_len) + ((1 + name_len) & 1);
        rpos = rpos.saturating_add(padded_name);
        if rpos + 4 > data.len() {
            break;
        }
        let block_size =
            u32::from_be_bytes([data[rpos], data[rpos + 1], data[rpos + 2], data[rpos + 3]])
                as usize;
        rpos += 4;
        let block_end = rpos.saturating_add(block_size).min(data.len());
        let block = &data[rpos..block_end];

        // 1005 (0x03ED) = ResolutionInfo (hRes Fixed 16.16 DPI at 0..4, vRes Fixed 16.16 DPI at 8..12)
        if res_id == 1005 && block.len() >= 12 {
            let h_fixed = u32::from_be_bytes([block[0], block[1], block[2], block[3]]);
            let v_fixed = u32::from_be_bytes([block[8], block[9], block[10], block[11]]);
            let h_dpi = h_fixed as f64 / 65536.0;
            let v_dpi = v_fixed as f64 / 65536.0;
            meta.dpi = format_dpi(h_dpi, v_dpi);
            break;
        }

        let padded_size = block_size.saturating_add(block_size & 1);
        let next = rpos.saturating_add(padded_size);
        if next <= rpos {
            break;
        }
        rpos = next;
    }
    meta
}

// ---------------------------------------------------------------------------
// TIFF: EXIF/IFD tags via kamadak-exif
// ---------------------------------------------------------------------------

fn inspect_tiff(bytes: &[u8]) -> TechnicalImageMeta {
    let mut meta = TechnicalImageMeta::default();
    let mut cursor = std::io::Cursor::new(bytes);
    if let Ok(exif_data) = exif::Reader::new().read_from_container(&mut cursor) {
        meta.dpi = exif_dpi_from_parsed(&exif_data);

        if let Some(f) = exif_data.get_field(exif::Tag::BitsPerSample, exif::In::PRIMARY) {
            if let Some(bpc) = f.value.get_uint(0) {
                meta.bits_per_channel = Some(if bpc == 32 {
                    "32-bit".to_string()
                } else {
                    format!("{bpc}-bit")
                });
            }
        }
        if let Some(f) = exif_data.get_field(exif::Tag::YCbCrSubSampling, exif::In::PRIMARY) {
            let h = f.value.get_uint(0).unwrap_or(1);
            let v = f.value.get_uint(1).unwrap_or(1);
            meta.subsampling = Some(
                match (h, v) {
                    (1, 1) => "4:4:4",
                    (2, 1) => "4:2:2",
                    (2, 2) => "4:2:0",
                    (1, 2) => "4:4:0",
                    (4, 1) => "4:1:1",
                    (4, 2) => "4:1:0",
                    _ => "None",
                }
                .to_string(),
            );
        } else {
            meta.subsampling = Some("None (RGB)".to_string());
        }
        if let Some(f) = exif_data.get_field(exif::Tag::Compression, exif::In::PRIMARY) {
            let comp = f.value.get_uint(0).unwrap_or(1);
            meta.estimated_quality = Some(
                match comp {
                    1 | 2 | 3 | 4 | 5 | 8 | 32773 | 32946 => "100% (Lossless)",
                    6 | 7 => "Lossy (JPEG-in-TIFF)",
                    _ => "TIFF",
                }
                .to_string(),
            );
        } else {
            meta.estimated_quality = Some("100% (Lossless)".to_string());
        }
    }
    meta
}

// ---------------------------------------------------------------------------
// AVIF: ISOBMFF `av1C` box parser for bit depth & chroma subsampling
// ---------------------------------------------------------------------------

fn inspect_avif(bytes: &[u8]) -> TechnicalImageMeta {
    let mut meta = TechnicalImageMeta::default();
    if let Some(pos) = bytes.windows(4).position(|w| w == b"av1C") {
        let cfg = &bytes[pos + 4..];
        if cfg.len() >= 4 {
            let seq_profile = (cfg[1] >> 5) & 0x07;
            let flags = cfg[2];
            let high_bitdepth = (flags & 0x40) != 0;
            let twelve_bit = (flags & 0x20) != 0;
            let monochrome = (flags & 0x10) != 0;
            let sub_x = (flags & 0x08) != 0;
            let sub_y = (flags & 0x04) != 0;

            let bpc = if seq_profile == 2 && high_bitdepth && twelve_bit {
                12
            } else if high_bitdepth {
                10
            } else {
                8
            };
            meta.bits_per_channel = Some(format!("{bpc}-bit"));
            meta.subsampling = Some(
                if monochrome {
                    "4:0:0 (Grayscale)"
                } else {
                    match (sub_x, sub_y) {
                        (false, false) => "4:4:4",
                        (true, false) => "4:2:2",
                        (true, true) => "4:2:0",
                        (false, true) => "4:4:0",
                    }
                }
                .to_string(),
            );
        }
    }
    meta.dpi = extract_exif_dpi(bytes);
    meta
}

// ---------------------------------------------------------------------------
// JXL: header bit depth & lossless check via jxl-oxide
// ---------------------------------------------------------------------------

fn inspect_jxl(bytes: &[u8]) -> TechnicalImageMeta {
    let mut meta = TechnicalImageMeta::default();
    if let Ok(jxl) = jxl_oxide::JxlImage::builder().read(std::io::Cursor::new(bytes)) {
        let hdr = jxl.image_header();
        let bps = hdr.metadata.bit_depth.bits_per_sample();
        let is_float = format!("{:?}", hdr.metadata.bit_depth).contains("Float");
        meta.bits_per_channel = Some(if is_float {
            format!("{bps}-bit (Float)")
        } else {
            format!("{bps}-bit")
        });
        meta.subsampling = Some("4:4:4".to_string());
        if !hdr.metadata.xyb_encoded {
            meta.estimated_quality = Some("100% (Lossless)".to_string());
        }
    }
    meta.dpi = extract_exif_dpi(bytes);
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn jpeg_quality_subsampling_bpc_and_png_dpi_detected() {
        // 1. Encode a JPEG at quality 85 and verify estimated quality, 8-bit, subsampling
        let mut rgb = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(64, 48);
        for (x, y, p) in rgb.enumerate_pixels_mut() {
            *p = Rgb([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8]);
        }
        let mut jpg_bytes = Vec::new();
        {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg_bytes, 85);
            image::ImageEncoder::write_image(
                enc,
                rgb.as_raw(),
                64,
                48,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        }
        let jpg_meta = inspect_bytes_technical_metadata(&jpg_bytes, "jpg");
        assert_eq!(jpg_meta.bits_per_channel.as_deref(), Some("8-bit"));
        assert!(jpg_meta.subsampling.is_some());
        assert_eq!(jpg_meta.estimated_quality.as_deref(), Some("85%"));

        // 2. Build a PNG with a 300 DPI pHYs chunk (11811 px/m) and verify DPI & lossless
        let mut png_bytes = Vec::new();
        {
            let enc = image::codecs::png::PngEncoder::new(&mut png_bytes);
            image::ImageEncoder::write_image(
                enc,
                rgb.as_raw(),
                64,
                48,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        }
        // Inject pHYs chunk right after IHDR (byte 33)
        let mut phys_chunk = Vec::new();
        phys_chunk.extend_from_slice(&9u32.to_be_bytes());
        phys_chunk.extend_from_slice(b"pHYs");
        phys_chunk.extend_from_slice(&11811u32.to_be_bytes());
        phys_chunk.extend_from_slice(&11811u32.to_be_bytes());
        phys_chunk.push(1u8); // meters
        phys_chunk.extend_from_slice(&0u32.to_be_bytes()); // CRC (not validated by header scanner)
        let mut png_with_phys = Vec::new();
        png_with_phys.extend_from_slice(&png_bytes[..33]);
        png_with_phys.extend_from_slice(&phys_chunk);
        png_with_phys.extend_from_slice(&png_bytes[33..]);

        let png_meta = inspect_bytes_technical_metadata(&png_with_phys, "png");
        assert_eq!(png_meta.bits_per_channel.as_deref(), Some("8-bit"));
        assert_eq!(png_meta.subsampling.as_deref(), Some("None (RGB)"));
        assert_eq!(png_meta.estimated_quality.as_deref(), Some("100% (Lossless)"));
        assert_eq!(png_meta.dpi.as_deref(), Some("300 DPI"));

        // 3. Encode a lossy WebP at Q=80 and lossless WebP and verify both
        let opts80 = oxideav_webp::EncodeOptions::default().with_quality(80.0);
        let webp_lossy = oxideav_webp::encode_rgb8(64, 48, rgb.as_raw(), &opts80).unwrap();
        let wl_meta = inspect_bytes_technical_metadata(&webp_lossy, "webp");
        assert_eq!(wl_meta.subsampling.as_deref(), Some("4:2:0"));
        assert_eq!(wl_meta.bits_per_channel.as_deref(), Some("8-bit"));
        assert!(
            wl_meta.estimated_quality.is_some(),
            "expected estimated WebP quality, got {:?}",
            wl_meta
        );
    }
}
