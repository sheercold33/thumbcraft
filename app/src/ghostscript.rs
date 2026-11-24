use crate::config::{
    GhostscriptConfig, GS_DPI_TO_PPM_FACTOR, GS_HEADER_PEEK_BYTES, GS_MAX_DPI, GS_MIN_DPI,
    GS_MIN_REVISION,
};
use image::DynamicImage;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsRenderMode {
    Thumbnail,
    Preview,
    FullView,
}

/// Clamps DPI to 50..=1440 matching plug_gs.tpp FUN_100012c0 (0x32..=0x5a0)
pub fn clamp_dpi(dpi: u32) -> u32 {
    dpi.clamp(GS_MIN_DPI, GS_MAX_DPI)
}

/// Converts DPI to pixels-per-meter matching plug_gs.tpp FUN_10001830 / FUN_10001c80:
/// `(int)((double)dpi * 39.37 + 0.5)`
pub fn dpi_to_pixels_per_meter(dpi: u32) -> u32 {
    ((dpi as f64) * GS_DPI_TO_PPM_FACTOR + 0.5) as u32
}

/// Checks if a Ghostscript version string meets minimum revision 800 (8.00+) from FUN_10001830
pub fn meets_min_revision(version: &str) -> bool {
    let mut parts = version.trim().split('.');
    let major: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let rev = major * 100 + minor.min(99);
    rev >= GS_MIN_REVISION
}

/// Identifies PDF/EPS/PostScript file signature from first 200 bytes matching plug_gs.tpp FUN_10001c80
pub fn identify_ps_pdf_header(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; GS_HEADER_PEEK_BYTES];
    let n = file.read(&mut buf).ok()?;
    let head = &buf[..n];
    if head.len() >= 8 && head[..5].eq_ignore_ascii_case(b"%PDF-") {
        let ver = String::from_utf8_lossy(&head[5..8]);
        return Some(format!("Adobe PDF version {ver}"));
    }
    if head.len() >= 4 && head[..4] == [0xC5, 0xD0, 0xD3, 0xC6] {
        return Some("Adobe EPSF (with preview)".to_string());
    }
    let text = String::from_utf8_lossy(head);
    if text.contains("%!PS-") {
        if text.contains(" EPSF-") {
            return Some("Adobe EPSF".to_string());
        }
        return Some("Adobe Postscript".to_string());
    }
    None
}

fn new_cmd(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Auto-detect Ghostscript CLI executable (gswin64c.exe, gswin32c.exe, gs)
pub fn detect_ghostscript() -> Option<String> {
    if let Ok(env_gs) = std::env::var("GS_PROG") {
        if Path::new(&env_gs).is_file() {
            return Some(env_gs);
        }
    }

    // Check standard Windows Ghostscript installation folders first (pick newest version)
    #[cfg(windows)]
    {
        let mut candidates: Vec<PathBuf> = Vec::new();
        for base in [
            r"C:\Program Files\gs",
            r"C:\Program Files (x86)\gs",
            r"C:\gs",
        ] {
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let bin = entry.path().join("bin");
                    for exe in ["gswin64c.exe", "gswin32c.exe"] {
                        let p = bin.join(exe);
                        if p.is_file() {
                            candidates.push(p);
                        }
                    }
                }
            }
        }
        candidates.sort();
        if let Some(best) = candidates.pop() {
            return Some(best.to_string_lossy().to_string());
        }
    }

    // Fallback to PATH
    for name in ["gswin64c", "gswin32c", "gs"] {
        if let Ok(output) = new_cmd(name).arg("--version").output() {
            if output.status.success() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Optional external CDR-to-PDF/EPS converter (Inkscape or LibreOffice) to feed into Ghostscript
pub fn detect_cdr_converter() -> Option<String> {
    #[cfg(windows)]
    {
        for p in [
            r"C:\Program Files\Inkscape\bin\inkscape.exe",
            r"C:\Program Files\LibreOffice\program\soffice.exe",
            r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
        ] {
            if Path::new(p).is_file() {
                return Some(p.to_string());
            }
        }
    }
    for name in ["inkscape", "soffice"] {
        if let Ok(output) = new_cmd(name).arg("--version").output() {
            if output.status.success() {
                return Some(name.to_string());
            }
        }
    }
    None
}

pub fn detect_version(gs_path: &str) -> Option<String> {
    if gs_path.trim().is_empty() {
        return None;
    }
    let out = new_cmd(gs_path).arg("--version").output().ok()?;
    if out.status.success() {
        let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !ver.is_empty() {
            return Some(ver);
        }
    }
    None
}

/// Count pages in PDF/PS/EPS using Ghostscript or fast header inspection
pub fn count_pages(config: &GhostscriptConfig, path: &Path) -> usize {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "eps" || ext == "epsi" || ext == "epsf" {
        return 1;
    }

    // Safe PDF page count via Ghostscript: pass file path through -sPDFFile (never interpolated into PS code)
    // and respect safer_mode using -dSAFER + --permit-file-read=<path>.
    if !config.gs_path.trim().is_empty() && (ext == "pdf" || ext == "ai") {
        let gs_path_str = path.to_string_lossy().replace('\\', "/");
        let mut cmd = new_cmd(&config.gs_path);
        cmd.args([
            "-dQUIET",
            "-dNODISPLAY",
            "-dBATCH",
            "-dNOPAUSE",
            "-dNOPAGEPROMPT",
        ]);
        if config.safer_mode {
            cmd.arg("-dSAFER");
            cmd.arg(format!("--permit-file-read={gs_path_str}"));
        }
        cmd.arg(format!("-sPDFFile={gs_path_str}"));
        cmd.args(["-c", "PDFFile (r) file runpdfbegin pdfpagecount = quit"]);
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                for tok in s.split_whitespace().rev() {
                    if let Ok(n) = tok.parse::<usize>() {
                        if (1..=9999).contains(&n) {
                            return n;
                        }
                    }
                }
            }
        }
    }

    // Fast fallback scan for %%Pages: (PostScript) or PDF /Type /Pages ... /Count N
    if let Ok(bytes) = std::fs::read(path) {
        let text = String::from_utf8_lossy(&bytes);
        if let Some(pos) = text.find("%%Pages:") {
            let rest = &text[pos + 8..];
            if let Some(tok) = rest.split_whitespace().next() {
                if let Ok(n) = tok.parse::<usize>() {
                    if n >= 1 {
                        return n;
                    }
                }
            }
        }
        if let Some(n) = count_pdf_pages_fallback(&text) {
            return n;
        }
    }
    1
}

fn count_pdf_pages_fallback(text: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut search = text;
    while let Some(idx) = search.find("/Type") {
        let after = &search[idx + 5..];
        let trimmed = after.trim_start();
        if trimmed.starts_with("/Pages") {
            let window_end = trimmed.len().min(256);
            let window = &trimmed[..window_end];
            if let Some(c_idx) = window.find("/Count") {
                let count_rest = window[c_idx + 6..].trim_start();
                let digits: String = count_rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(n) = digits.parse::<usize>() {
                    if (1..=9999).contains(&n) {
                        best = Some(best.map_or(n, |cur| cur.max(n)));
                    }
                }
            }
        }
        search = &search[idx + 5..];
    }
    best
}

/// Render a page of PDF/AI/EPS/PS/PRN using Ghostscript with exact plug_gs.tpp parameters
pub fn render_page(
    config: &GhostscriptConfig,
    path: &Path,
    page: usize,
    mode: GsRenderMode,
) -> Result<DynamicImage, String> {
    if config.gs_path.trim().is_empty() {
        return Err("Ghostscript executable not configured".to_string());
    }

    let base_dpi = match mode {
        GsRenderMode::Thumbnail => clamp_dpi(config.thumbnail_dpi),
        GsRenderMode::Preview => clamp_dpi(config.preview_dpi),
        GsRenderMode::FullView => clamp_dpi(config.full_view_dpi),
    };
    let dpi = effective_gs_dpi(path, base_dpi, mode);

    // Match plug_gs.tpp 0x10001ad5: antialias bits forced to 1 for thumbnails
    let aa_bits = match mode {
        GsRenderMode::Thumbnail => 1,
        GsRenderMode::Preview | GsRenderMode::FullView => match config.antialias_bits {
            1 | 2 | 4 => config.antialias_bits,
            _ => 4,
        },
    };

    let device = match config.color_depth {
        1 => "pngmono",
        8 => "pnggray",
        _ => "png16m",
    };

    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let out_path = std::env::temp_dir().join(format!(
        "thumbcraft_gs_{}_{}.png",
        std::process::id(),
        seq
    ));

    let page_num = page.max(1);
    let mut cmd = new_cmd(&config.gs_path);
    if config.safer_mode {
        cmd.arg("-dSAFER");
    }
    cmd.args([
        "-dQUIET",
        "-dBATCH",
        "-dNOPAUSE",
        "-dNOPAGEPROMPT",
        &format!("-sDEVICE={device}"),
        &format!("-r{dpi}"),
        &format!("-dFirstPage={page_num}"),
        &format!("-dLastPage={page_num}"),
        &format!("-dTextAlphaBits={aa_bits}"),
        &format!("-dGraphicsAlphaBits={aa_bits}"),
    ]);
    if config.eps_crop {
        cmd.arg("-dEPSCrop");
    }
    // Match plug_gs.tpp FUN_10001830 (flags & 0x41 || color_depth == 1):
    // disable interpolation for Thumbnail and Preview modes (or 1-bit depth); enable for FullView.
    if matches!(mode, GsRenderMode::Thumbnail | GsRenderMode::Preview) || config.color_depth == 1 {
        cmd.arg("-dNOINTERPOLATE");
    } else {
        cmd.arg("-dDOINTERPOLATE");
    }
    cmd.arg(format!("-sOutputFile={}", out_path.to_string_lossy()));
    cmd.arg(path);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to launch Ghostscript: {e}"))?;

    if !output.status.success() || !out_path.is_file() {
        let _ = std::fs::remove_file(&out_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Ghostscript failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }

    let png_bytes = std::fs::read(&out_path).map_err(|e| format!("GS PNG read error: {e}"));
    let _ = std::fs::remove_file(&out_path);
    let png_bytes = png_bytes?;
    crate::decoder::decode_image_with_limits(&png_bytes, Some(image::ImageFormat::Png))
        .map_err(|e| format!("GS PNG decode error: {e}"))
}

/// Scales down Ghostscript DPI when an EPS/PS document has a gigantic physical BoundingBox
/// (e.g., billboard/banner artwork) so thumbnails & previews stay fast and never OOM.
fn effective_gs_dpi(path: &Path, base_dpi: u32, mode: GsRenderMode) -> u32 {
    let max_side_px: f64 = match mode {
        GsRenderMode::Thumbnail => 2048.0,
        GsRenderMode::Preview => 4096.0,
        GsRenderMode::FullView => 8192.0,
    };
    let Some((w_pts, h_pts)) = peek_ps_bounding_box_pts(path) else {
        return base_dpi;
    };
    let max_inches = (w_pts.max(h_pts)) / 72.0;
    if max_inches <= 0.1 {
        return base_dpi;
    }
    let projected_px = max_inches * (base_dpi as f64);
    if projected_px <= max_side_px {
        return base_dpi;
    }
    ((max_side_px / max_inches).floor() as u32).clamp(4, base_dpi)
}

fn peek_ps_bounding_box_pts(path: &Path) -> Option<(f64, f64)> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; 32 * 1024];
    let n = file.read(&mut head).ok()?;
    head.truncate(n);
    let text = String::from_utf8_lossy(&head);
    for prefix in ["%%HiResBoundingBox:", "%%BoundingBox:"] {
        for line in text.lines().take(250) {
            if let Some(rest) = line.strip_prefix(prefix) {
                let nums: Vec<f64> = rest
                    .split_whitespace()
                    .filter_map(|t| t.parse::<f64>().ok())
                    .collect();
                if nums.len() >= 4 {
                    let w = (nums[2] - nums[0]).abs();
                    let h = (nums[3] - nums[1]).abs();
                    if w > 1.0 && h > 1.0 {
                        return Some((w, h));
                    }
                }
            }
        }
    }
    None
}

/// Render raw PostScript/EPS/PDF bytes (e.g. extracted from inside a CDR file) via Ghostscript
pub fn render_bytes(
    config: &GhostscriptConfig,
    bytes: &[u8],
    ext: &str,
    page: usize,
    mode: GsRenderMode,
) -> Result<DynamicImage, String> {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let in_path = std::env::temp_dir().join(format!(
        "thumbcraft_gs_in_{}_{}.{}",
        std::process::id(),
        seq,
        ext
    ));
    std::fs::write(&in_path, bytes).map_err(|e| e.to_string())?;
    let res = render_page(config, &in_path, page, mode);
    let _ = std::fs::remove_file(&in_path);
    res
}

/// Optional pipeline: convert CDR -> PDF using configured external converter (Inkscape/LibreOffice),
/// then render the resulting PDF with Ghostscript!
pub fn render_cdr_via_converter_and_gs(
    config: &GhostscriptConfig,
    cdr_path: &Path,
    page: usize,
    mode: GsRenderMode,
) -> Result<DynamicImage, String> {
    let conv = config.cdr_converter_path.trim();
    if conv.is_empty() || config.gs_path.trim().is_empty() {
        return Err("No external CDR converter configured".to_string());
    }
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_dir = std::env::temp_dir().join(format!("thumbcraft_cdr_{}_{}", std::process::id(), seq));
    let _ = std::fs::create_dir_all(&tmp_dir);
    let pdf_path = tmp_dir.join("converted.pdf");

    let lower = conv.to_ascii_lowercase();
    let status = if lower.contains("inkscape") {
        new_cmd(conv)
            .arg(cdr_path)
            .arg("--export-type=pdf")
            .arg(format!("--export-filename={}", pdf_path.to_string_lossy()))
            .status()
    } else {
        new_cmd(conv)
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(&tmp_dir)
            .arg(cdr_path)
            .status()
    };

    let result = match status {
        Ok(st) if st.success() => {
            let actual_pdf = if pdf_path.is_file() {
                pdf_path.clone()
            } else {
                let stem = cdr_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("converted");
                tmp_dir.join(format!("{stem}.pdf"))
            };
            if actual_pdf.is_file() {
                render_page(config, &actual_pdf, page, mode)
            } else {
                Err("CDR converter did not output PDF".to_string())
            }
        }
        Ok(st) => Err(format!("CDR converter exited with {st}")),
        Err(e) => Err(format!("Failed to launch CDR converter: {e}")),
    };

    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

/// Extract keywords and document info from PDF, AI, EPS, and PS files
pub fn extract_ps_pdf_metadata(path: &Path) -> (Vec<String>, BTreeMap<String, String>) {
    let mut keywords = Vec::new();
    let mut meta = BTreeMap::new();

    let Ok(bytes) = std::fs::read(path) else {
        return (keywords, meta);
    };

    // Limit header/trailer scan to avoid choking on huge binary streams
    let len = bytes.len();
    let mut sample = String::new();
    let head_len = len.min(65536);
    sample.push_str(&String::from_utf8_lossy(&bytes[..head_len]));
    if len > head_len {
        let tail_start = len.saturating_sub(65536);
        sample.push('\n');
        sample.push_str(&String::from_utf8_lossy(&bytes[tail_start..]));
    }

    // 1. Embedded XMP packet
    if let Some((xmp_kws, xmp_meta)) = crate::cdr::extract_embedded_xmp(&bytes) {
        keywords.extend(xmp_kws);
        meta.extend(xmp_meta);
    }

    // 2. EPS / PostScript DSC comments
    for line in sample.lines().take(200) {
        if let Some(rest) = line.strip_prefix("%%Title:") {
            let v = rest.trim().trim_matches(|c| c == '(' || c == ')');
            if !v.is_empty() {
                meta.entry("Title".into()).or_insert_with(|| v.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("%%Creator:") {
            let v = rest.trim().trim_matches(|c| c == '(' || c == ')');
            if !v.is_empty() {
                meta.entry("Creator".into())
                    .or_insert_with(|| v.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("%%BoundingBox:") {
            let v = rest.trim();
            if !v.is_empty() && !v.contains("atend") {
                meta.entry("BoundingBox".into())
                    .or_insert_with(|| v.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("%%Keywords:") {
            crate::cdr::push_split_keywords(&mut keywords, rest);
        }
    }

    // 3. PDF Info dictionary fields (/Title (...), /Author (...), /Subject (...), /Keywords (...))
    for (pdf_key, label) in [
        ("/Title", "Title"),
        ("/Author", "Author"),
        ("/Subject", "Subject"),
        ("/Creator", "Creator"),
        ("/Producer", "Producer"),
        ("/Keywords", "Keywords"),
    ] {
        if let Some(val) = extract_pdf_dict_string(&sample, pdf_key) {
            if pdf_key == "/Keywords" {
                crate::cdr::push_split_keywords(&mut keywords, &val);
            } else {
                meta.entry(label.into()).or_insert(val);
            }
        }
    }

    crate::cdr::dedup_keywords(&mut keywords);
    (keywords, meta)
}

fn extract_pdf_dict_string(text: &str, key: &str) -> Option<String> {
    let idx = text.find(key)?;
    let rest = text[idx + key.len()..].trim_start();
    if let Some(after_paren) = rest.strip_prefix('(') {
        let mut out = String::new();
        let mut escaped = false;
        let mut depth = 1;
        for ch in after_paren.chars() {
            if escaped {
                out.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '(' {
                depth += 1;
            } else if ch == ')' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            out.push(ch);
        }
        let trimmed = out.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_pages_pdf_fallback_and_parentheses_filename_safe() {
        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_gs_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let sample_dir = crate::sample_gen::ensure_sample_library(&tmp);
        let orig_pdf = sample_dir.join("04_Brand_Guidelines.pdf");
        // Copy to a filename containing parentheses & PS tokens to verify zero PostScript injection
        let tricky_pdf = tmp.join("brand (copy) ) quit (.pdf");
        std::fs::copy(&orig_pdf, &tricky_pdf).unwrap();

        // 1. Safe Ghostscript invocation (if installed) with safer_mode = true
        let cfg = GhostscriptConfig::default();
        assert!(cfg.safer_mode);
        let pages = count_pages(&cfg, &tricky_pdf);
        assert_eq!(pages, 2);

        // 2. Fallback pure-Rust PDF /Pages /Count parser when GS is disabled
        let no_gs = GhostscriptConfig {
            gs_path: String::new(),
            ..Default::default()
        };
        assert_eq!(count_pages(&no_gs, &tricky_pdf), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
