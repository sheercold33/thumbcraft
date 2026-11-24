use image::{DynamicImage, ImageBuffer, Rgba};
use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read};
use std::path::Path;

#[derive(Debug)]
pub struct CdrParsed {
    pub version_label: String,
    pub preview: Option<DynamicImage>,
    pub keywords: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub embedded_ps_pdf: Option<(Vec<u8>, &'static str)>,
    pub page_count: usize,
}

/// Parses CorelDRAW files across all versions:
/// - ZIP-based CDR (X4/v14 through 2024+): extracts PNG/BMP preview + META-INF/metadata.xml & XMP keywords + nested riffData.cdr
/// - Classic RIFF CDR/CMX/CCX/PAT (v3-v13): extracts DISP/BM DIB preview + LIST/INFO (IKEY, INAM, ISBJ, ICMT, IART, ISFT, ICRD) + XMP + embedded PS/EPS
/// - Legacy CorelDRAW v2 (WL) and @CorelBMF previews
#[allow(dead_code)]
pub fn parse_cdr(path: &Path) -> Result<CdrParsed, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read CDR: {e}"))?;
    parse_cdr_bytes(&bytes)
}

pub fn parse_cdr_bytes(bytes: &[u8]) -> Result<CdrParsed, String> {
    if bytes.len() < 12 {
        return Err("File too small to be a CorelDRAW document".to_string());
    }

    // 1. ZIP-based CorelDRAW X4+ (PK\x03\x04)
    if bytes.starts_with(b"PK\x03\x04") {
        return parse_zip_cdr(bytes);
    }

    // 2. Classic RIFF-based CorelDRAW v3-v13 (RIFF)
    if bytes.starts_with(b"RIFF") {
        return parse_riff_cdr(bytes);
    }

    // 3. CorelDRAW Version 2 (WL = 0x4c57)
    if bytes[0] == b'W' && bytes[1] == b'L' {
        return parse_wl_v2_cdr(bytes);
    }

    // 4. Corel Gallery @CorelBMF
    if bytes.starts_with(b"@CorelBMF") {
        return parse_corel_bmf(bytes);
    }

    // 5. Tolerate prefixed containers (e.g. MacBinary 128-byte header or leading junk before RIFF/ZIP/@CorelBMF)
    let head_scan = bytes.len().min(512);
    if let Some(pos) = find_subslice(&bytes[..head_scan], b"PK\x03\x04") {
        if pos > 0 {
            return parse_zip_cdr(&bytes[pos..]);
        }
    }
    if let Some(pos) = find_subslice(&bytes[..head_scan], b"RIFF") {
        if pos > 0 {
            return parse_riff_cdr(&bytes[pos..]);
        }
    }
    if let Some(pos) = find_subslice(&bytes[..head_scan], b"@CorelBMF") {
        if pos > 0 {
            return parse_corel_bmf(&bytes[pos..]);
        }
    }

    // 6. OLE Compound File (D0 CF 11 E0 A1 B1 1A E1) or legacy container with embedded RIFF/PNG/BMP preview
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        if let Some(pos) = find_subslice(bytes, b"RIFF") {
            if let Ok(parsed) = parse_riff_cdr(&bytes[pos..]) {
                if parsed.preview.is_some() || !parsed.keywords.is_empty() {
                    return Ok(parsed);
                }
            }
        }
        if let Some(img) = scan_embedded_preview_image(bytes) {
            let mut metadata = BTreeMap::new();
            metadata.insert("Container".into(), "OLE Compound CDR".into());
            metadata.insert("Preview Source".into(), "Embedded OLE preview".into());
            return Ok(CdrParsed {
                version_label: "CorelDRAW (OLE)".into(),
                preview: Some(img),
                keywords: Vec::new(),
                metadata,
                embedded_ps_pdf: None,
                page_count: 1,
            });
        }
    }

    Err("Unrecognized CorelDRAW container header".to_string())
}

fn parse_zip_cdr(bytes: &[u8]) -> Result<CdrParsed, String> {
    let cursor = Cursor::new(bytes);
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(zip_err) => {
            // Many damaged or trailing-padded CorelDRAW X4+ ZIPs lack a valid EOCD record,
            // while their PK\x03\x04 Local File Headers (including thumbnails & metadata.xml) are intact.
            if let Some(recovered) = parse_zip_cdr_local_scan(bytes) {
                return Ok(recovered);
            }
            return Err(format!("Invalid ZIP-based CDR: {zip_err}"));
        }
    };

    let mut preview: Option<DynamicImage> = None;
    let mut keywords: Vec<String> = Vec::new();
    let mut metadata: BTreeMap<String, String> = BTreeMap::new();
    let mut version_label = "CorelDRAW (ZIP X4+)".to_string();
    let mut embedded_ps_pdf: Option<(Vec<u8>, &'static str)> = None;
    let mut page_thumbs = 0usize;

    let mut names: Vec<String> = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            names.push(file.name().to_string());
        }
    }

    // Priority preview candidates (prefer well-proportioned previews; CorelDRAW sometimes writes
    // degenerate 30000x256 strips into thumbnail.bmp on blank templates while page1.bmp is 256x170)
    let preview_candidates = [
        "metadata/thumbnails/thumbnail.png",
        "previews/thumbnail.png",
        "metadata/thumbnails/page1.png",
        "previews/page1.png",
        "thumbnail.png",
        "metadata/thumbnails/thumbnail.bmp",
        "previews/thumbnail.bmp",
        "metadata/thumbnails/page1.bmp",
        "previews/page1.bmp",
    ];

    let is_degenerate_preview = |img: &DynamicImage| -> bool {
        let w = img.width();
        let h = img.height();
        if w == 0 || h == 0 {
            return true;
        }
        let max_d = w.max(h);
        let min_d = w.min(h).max(1);
        max_d > 4096 || (max_d / min_d) >= 16
    };

    let mut fallback_preview: Option<(DynamicImage, String)> = None;
    for cand in preview_candidates {
        if let Some(actual) = names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(cand))
            .cloned()
        {
            if let Ok(mut f) = archive.by_name(&actual) {
                if f.size() <= 64 * 1024 * 1024 {
                    let mut buf = Vec::new();
                    if f.read_to_end(&mut buf).is_ok() {
                        if let Ok(img) = image::load_from_memory(&buf) {
                            if is_degenerate_preview(&img) {
                                if fallback_preview.is_none() {
                                    fallback_preview = Some((img, actual));
                                }
                            } else {
                                preview = Some(img);
                                metadata.insert("Preview Source".into(), actual);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: any image under thumbnails/ or previews/
    for name in &names {
        let lower = name.to_ascii_lowercase();
        if (lower.contains("thumbnail") || lower.contains("preview"))
            && (lower.ends_with(".png")
                || lower.ends_with(".bmp")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg"))
        {
            if lower.contains("page") {
                page_thumbs += 1;
            }
            if preview.is_none() {
                if let Ok(mut f) = archive.by_name(name) {
                    if f.size() <= 64 * 1024 * 1024 {
                        let mut buf = Vec::new();
                        if f.read_to_end(&mut buf).is_ok() {
                            if let Ok(img) = image::load_from_memory(&buf) {
                                if is_degenerate_preview(&img) {
                                    if fallback_preview.is_none() {
                                        fallback_preview = Some((img, name.clone()));
                                    }
                                } else {
                                    preview = Some(img);
                                    metadata.insert("Preview Source".into(), name.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if preview.is_none() {
        if let Some((img, src)) = fallback_preview {
            preview = Some(img);
            metadata.insert("Preview Source".into(), src);
        }
    }

    // Parse all XML metadata files (META-INF/metadata.xml, content/metadata.xml, docProps/core.xml, etc.)
    for name in &names {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".xml") || lower.ends_with(".xmp") || lower.ends_with(".rdf") {
            if lower.contains("metadata")
                || lower.contains("core")
                || lower.contains("info")
                || lower.contains("manifest")
                || lower.contains("xmp")
            {
                if let Ok(mut f) = archive.by_name(name) {
                    if f.size() <= 4 * 1024 * 1024 {
                        let mut buf = Vec::new();
                        if f.read_to_end(&mut buf).is_ok() {
                            let xml = String::from_utf8_lossy(&buf);
                            let (kws, meta) = parse_xmp_or_xml_metadata(&xml);
                            if !kws.is_empty() {
                                metadata.insert("Keyword Source".into(), name.clone());
                            }
                            keywords.extend(kws);
                            for (k, v) in meta {
                                metadata.entry(k).or_insert(v);
                            }
                            if lower.contains("manifest") {
                                if let Some(ver) = detect_version_from_manifest(&xml) {
                                    version_label = ver;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Also inspect nested content/riffData.cdr for additional INFO/IKEY keywords, version, or fallback DISP preview
    for name in &names {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with("riffdata.cdr") || lower.ends_with(".cdr") {
            if let Ok(mut f) = archive.by_name(name) {
                if f.size() <= 32 * 1024 * 1024 {
                    let mut buf = Vec::new();
                    if f.read_to_end(&mut buf).is_ok() && buf.starts_with(b"RIFF") {
                        if let Ok(riff) = parse_riff_cdr(&buf) {
                            if version_label == "CorelDRAW (ZIP X4+)" {
                                version_label = format!("{} (ZIP)", riff.version_label);
                            }
                            if preview.is_none() {
                                preview = riff.preview;
                            }
                            if embedded_ps_pdf.is_none() {
                                embedded_ps_pdf = riff.embedded_ps_pdf;
                            }
                            keywords.extend(riff.keywords);
                            for (k, v) in riff.metadata {
                                metadata.entry(k).or_insert(v);
                            }
                        }
                    }
                }
            }
        }
    }

    metadata.insert("Container".into(), "ZIP (CorelDRAW X4+)".into());
    metadata.insert("CDR Version".into(), version_label.clone());
    dedup_keywords(&mut keywords);

    Ok(CdrParsed {
        version_label,
        preview,
        keywords,
        metadata,
        embedded_ps_pdf,
        page_count: page_thumbs.max(1),
    })
}

fn detect_version_from_manifest(xml: &str) -> Option<String> {
    if xml.contains("application/x-vnd.corel.graphics.draw.document+zip") {
        return Some("CorelDRAW X4+".to_string());
    }
    let search_area = xml
        .find("?>")
        .map(|p| &xml[p + 2..])
        .unwrap_or(xml);
    if let Some(idx) = search_area.find("version=\"") {
        let rest = &search_area[idx + 9..];
        if let Some(end) = rest.find('"') {
            let v = &rest[..end];
            if !v.is_empty() {
                return Some(format!("CorelDRAW (v{v})"));
            }
        }
    }
    None
}

fn parse_riff_cdr(bytes: &[u8]) -> Result<CdrParsed, String> {
    let mut preview: Option<DynamicImage> = None;
    let mut keywords: Vec<String> = Vec::new();
    let mut metadata: BTreeMap<String, String> = BTreeMap::new();
    let mut embedded_ps_pdf: Option<(Vec<u8>, &'static str)> = None;

    let form_type = if bytes.len() >= 12 {
        String::from_utf8_lossy(&bytes[8..12]).to_string()
    } else {
        "CDR ".to_string()
    };
    let version_label = riff_form_to_version(&form_type);
    metadata.insert("Container".into(), format!("RIFF ({form_type})"));
    metadata.insert("CDR Version".into(), version_label.clone());

    // Walk full RIFF chunk hierarchy
    walk_riff_chunks(
        bytes,
        12,
        bytes.len(),
        None,
        &mut preview,
        &mut keywords,
        &mut metadata,
        &mut embedded_ps_pdf,
        0,
    );

    // Fallback scan for DISP or BM near the start of the RIFF stream
    if preview.is_none() {
        let scan_end = bytes.len().min(65536);
        if let Some(pos) = find_subslice(&bytes[..scan_end], b"DISP") {
            if pos + 12 < bytes.len() {
                let chunk_size = u32::from_le_bytes([
                    bytes[pos + 4],
                    bytes[pos + 5],
                    bytes[pos + 6],
                    bytes[pos + 7],
                ]) as usize;
                let data_start = pos + 8;
                let data_end = data_start.saturating_add(chunk_size).min(bytes.len());
                if let Some(img) = decode_disp_chunk(&bytes[data_start..data_end]) {
                    preview = Some(img);
                    metadata.insert("Preview Source".into(), "RIFF DISP (header scan)".into());
                }
            }
        }
    }

    if preview.is_none() {
        if let Some(img) = scan_embedded_preview_image(bytes) {
            preview = Some(img);
            metadata.insert("Preview Source".into(), "RIFF embedded bitmap".into());
        }
    }

    // Scan for embedded XMP packet anywhere in the RIFF file
    if let Some((xmp_kws, xmp_meta)) = extract_embedded_xmp(bytes) {
        keywords.extend(xmp_kws);
        for (k, v) in xmp_meta {
            metadata.entry(k).or_insert(v);
        }
    }

    // Check for embedded PostScript/EPS stream for Ghostscript fallback
    if embedded_ps_pdf.is_none() {
        if let Some(ps_start) = find_subslice(bytes, b"%!PS-Adobe") {
            let slice = &bytes[ps_start..];
            let ps_end = find_subslice(slice, b"%%EOF")
                .map(|p| (p + 5).min(slice.len()))
                .unwrap_or_else(|| slice.len().min(2 * 1024 * 1024));
            if ps_end > 32 {
                embedded_ps_pdf = Some((slice[..ps_end].to_vec(), "eps"));
            }
        }
    }

    dedup_keywords(&mut keywords);
    Ok(CdrParsed {
        version_label,
        preview,
        keywords,
        metadata,
        embedded_ps_pdf,
        page_count: 1,
    })
}

#[allow(clippy::too_many_arguments)]
fn walk_riff_chunks(
    data: &[u8],
    start: usize,
    end: usize,
    parent_list_type: Option<[u8; 4]>,
    preview: &mut Option<DynamicImage>,
    keywords: &mut Vec<String>,
    metadata: &mut BTreeMap<String, String>,
    embedded_ps_pdf: &mut Option<(Vec<u8>, &'static str)>,
    depth: usize,
) {
    if depth > 10 {
        return;
    }
    let mut pos = start;
    while pos + 8 <= end {
        let fourcc = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
        let size = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        let payload_start = pos + 8;
        let payload_end = payload_start.saturating_add(size).min(end);
        let payload = &data[payload_start..payload_end];

        if &fourcc == b"LIST" || &fourcc == b"RIFF" {
            if payload.len() >= 4 {
                let list_type = [payload[0], payload[1], payload[2], payload[3]];
                walk_riff_chunks(
                    data,
                    payload_start + 4,
                    payload_end,
                    Some(list_type),
                    preview,
                    keywords,
                    metadata,
                    embedded_ps_pdf,
                    depth + 1,
                );
            }
        } else if &fourcc == b"DISP" && preview.is_none() {
            if let Some(img) = decode_disp_chunk(payload) {
                *preview = Some(img);
                metadata.insert("Preview Source".into(), "RIFF DISP bitmap".into());
            }
        } else if &fourcc == b"bmpt" && preview.is_none() {
            if let Ok(img) = crate::decoder::decode_image_with_limits(payload, None) {
                *preview = Some(img);
                metadata.insert("Preview Source".into(), "RIFF bmpt".into());
            }
        } else if &fourcc == b"bmp " && preview.is_none() {
            if let Some(img) = decode_disp_chunk(payload) {
                *preview = Some(img);
                metadata.insert("Preview Source".into(), "RIFF bmp".into());
            }
        } else if parent_list_type == Some(*b"INFO") {
            let text = decode_riff_text(payload);
            if !text.is_empty() {
                match &fourcc {
                    b"IKEY" => {
                        push_split_keywords(keywords, &text);
                        metadata.insert("Keyword Source".into(), "RIFF LIST/INFO/IKEY".into());
                    }
                    b"INAM" => {
                        metadata.entry("Title".into()).or_insert(text);
                    }
                    b"ISBJ" => {
                        metadata.entry("Subject".into()).or_insert(text);
                    }
                    b"ICMT" => {
                        metadata.entry("Comments".into()).or_insert(text);
                    }
                    b"IART" => {
                        metadata.entry("Author".into()).or_insert(text);
                    }
                    b"ISFT" => {
                        metadata.entry("Software".into()).or_insert(text);
                    }
                    b"ICRD" => {
                        metadata.entry("Created".into()).or_insert(text);
                    }
                    b"ICOP" => {
                        metadata.entry("Copyright".into()).or_insert(text);
                    }
                    _ => {}
                }
            }
        } else if &fourcc == b"IKEY" {
            let text = decode_riff_text(payload);
            if !text.is_empty() {
                push_split_keywords(keywords, &text);
                metadata.insert("Keyword Source".into(), "RIFF IKEY".into());
            }
        } else if &fourcc == b"udta" || &fourcc == b"sumi" || &fourcc == b"os2i" {
            extract_summary_chunk_keywords(payload, keywords, metadata);
        } else if (&fourcc == b"eps " || &fourcc == b"ps  ") && embedded_ps_pdf.is_none() {
            if payload.len() > 24 {
                *embedded_ps_pdf = Some((payload.to_vec(), "eps"));
            }
        }

        // RIFF chunks are word-aligned (padded to even byte boundary)
        let padded = size.saturating_add(size & 1);
        let next = payload_start.saturating_add(padded);
        if next <= pos {
            break;
        }
        pos = next;
    }
}

/// Decodes a RIFF DISP chunk into a DynamicImage:
/// 4-byte clipboard type (usually 8 = CF_DIB) followed by BITMAPINFOHEADER (40 bytes) + palette + pixel bits.
fn decode_disp_chunk(payload: &[u8]) -> Option<DynamicImage> {
    if payload.len() < 44 {
        return None;
    }

    // Try direct BMP if payload starts with "BM"
    if payload.starts_with(b"BM") {
        if let Some(img) = crate::decoder::decode_bmp_tolerant(payload) {
            return Some(img);
        }
    }

    let is_dib_hdr = |off: usize| -> bool {
        if off + 4 > payload.len() {
            return false;
        }
        let sz = u32::from_le_bytes([
            payload[off],
            payload[off + 1],
            payload[off + 2],
            payload[off + 3],
        ]);
        matches!(sz, 40 | 52 | 56 | 64 | 108 | 124)
    };

    // Standard RIFF DISP: 4-byte format header (CF_DIB = 8) followed by BITMAPINFOHEADER/V4/V5
    // Also check small offsets (0, 8, 12, 16, 40, 44) used by CorelDRAW bmp/DISP variants.
    for off in [4usize, 0, 8, 12, 16, 40, 44] {
        if is_dib_hdr(off) {
            if let Some(img) = wrap_dib_as_bmp(&payload[off..]) {
                return Some(img);
            }
        }
    }
    None
}

fn wrap_dib_as_bmp(dib: &[u8]) -> Option<DynamicImage> {
    if dib.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]);
    if !matches!(header_size, 40 | 52 | 56 | 64 | 108 | 124) {
        return None;
    }
    let bpp = u16::from_le_bytes([dib[14], dib[15]]);
    let colors_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]);
    let palette_entries = if bpp <= 8 && bpp > 0 {
        if colors_used > 0 {
            colors_used.min(256)
        } else {
            1u32 << bpp
        }
    } else {
        colors_used.min(256)
    };
    let pixel_offset = 14u32
        .saturating_add(header_size)
        .saturating_add(palette_entries.saturating_mul(4));
    let file_size = 14u32.saturating_add(dib.len() as u32);

    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&pixel_offset.to_le_bytes());
    bmp.extend_from_slice(dib);

    crate::decoder::decode_bmp_tolerant(&bmp)
}

/// Fallback ZIP CDR parser that scans `PK\x03\x04` Local File Headers directly when the ZIP EOCD
/// central directory is missing, truncated, or has trailing padding.
fn parse_zip_cdr_local_scan(bytes: &[u8]) -> Option<CdrParsed> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut pos = 0usize;

    while pos + 30 <= bytes.len() {
        let Some(rel) = find_subslice(&bytes[pos..], b"PK\x03\x04") else {
            break;
        };
        let hdr = pos + rel;
        if hdr + 30 > bytes.len() {
            break;
        }
        let flags = u16::from_le_bytes([bytes[hdr + 6], bytes[hdr + 7]]);
        let method = u16::from_le_bytes([bytes[hdr + 8], bytes[hdr + 9]]);
        let comp_size = u32::from_le_bytes([
            bytes[hdr + 18],
            bytes[hdr + 19],
            bytes[hdr + 20],
            bytes[hdr + 21],
        ]) as usize;
        let name_len = u16::from_le_bytes([bytes[hdr + 26], bytes[hdr + 27]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[hdr + 28], bytes[hdr + 29]]) as usize;

        let name_start = hdr + 30;
        let data_start = name_start.saturating_add(name_len).saturating_add(extra_len);
        if data_start > bytes.len() || name_len == 0 || name_len > 512 {
            pos = hdr + 4;
            continue;
        }
        let name = String::from_utf8_lossy(&bytes[name_start..name_start + name_len]).to_string();
        let lower = name.to_ascii_lowercase();
        let interesting = lower.contains("thumbnail")
            || lower.contains("preview")
            || lower.ends_with(".xml")
            || lower.ends_with(".xmp")
            || lower.ends_with(".rdf")
            || lower.ends_with(".cdr")
            || lower.ends_with(".png")
            || lower.ends_with(".bmp")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg");

        let has_data_desc = (flags & 0x0008) != 0;
        let avail_end = if !has_data_desc && comp_size > 0 {
            data_start.saturating_add(comp_size).min(bytes.len())
        } else {
            find_subslice(&bytes[data_start..], b"PK\x03\x04")
                .or_else(|| find_subslice(&bytes[data_start..], b"PK\x01\x02"))
                .map(|r| data_start + r)
                .unwrap_or(bytes.len())
        };

        if interesting && data_start < avail_end {
            let slice = &bytes[data_start..avail_end];
            let mut decoded = Vec::new();
            let ok = match method {
                0 => {
                    decoded.extend_from_slice(&slice[..slice.len().min(32 * 1024 * 1024)]);
                    true
                }
                8 => {
                    let mut dec = flate2::read::DeflateDecoder::new(slice);
                    let mut limited = dec.by_ref().take(32 * 1024 * 1024);
                    let _ = limited.read_to_end(&mut decoded);
                    !decoded.is_empty()
                }
                _ => false,
            };
            if ok && !decoded.is_empty() {
                entries.push((name, decoded));
            }
        }

        pos = if !has_data_desc && comp_size > 0 && data_start + comp_size > hdr {
            data_start + comp_size
        } else {
            data_start.max(hdr + 4)
        };
    }

    let is_degenerate = |img: &DynamicImage| -> bool {
        let w = img.width();
        let h = img.height();
        w == 0 || h == 0 || w.max(h) > 4096 || (w.max(h) / w.min(h).max(1)) >= 16
    };

    let mut preview: Option<DynamicImage> = None;
    let mut fallback_preview: Option<(DynamicImage, String)> = None;
    let mut keywords: Vec<String> = Vec::new();
    let mut metadata: BTreeMap<String, String> = BTreeMap::new();
    let mut version_label = "CorelDRAW (ZIP Recovered)".to_string();
    let mut embedded_ps_pdf: Option<(Vec<u8>, &'static str)> = None;

    for (name, buf) in &entries {
        let lower = name.to_ascii_lowercase();
        if (lower.contains("thumbnail") || lower.contains("preview") || entries.len() <= 4)
            && (lower.ends_with(".png")
                || lower.ends_with(".bmp")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg"))
        {
            if let Some((img, _)) = crate::decoder::decode_by_magic_bytes(buf) {
                if is_degenerate(&img) {
                    if fallback_preview.is_none() {
                        fallback_preview = Some((img, name.clone()));
                    }
                } else if preview.is_none() {
                    preview = Some(img);
                    metadata.insert("Preview Source".into(), format!("{name} (ZIP local scan)"));
                }
            }
        } else if lower.ends_with(".xml") || lower.ends_with(".xmp") || lower.ends_with(".rdf") {
            let xml = String::from_utf8_lossy(buf);
            let (kws, meta) = parse_xmp_or_xml_metadata(&xml);
            keywords.extend(kws);
            for (k, v) in meta {
                metadata.entry(k).or_insert(v);
            }
            if lower.contains("manifest") {
                if let Some(ver) = detect_version_from_manifest(&xml) {
                    version_label = ver;
                }
            }
        } else if (lower.ends_with("riffdata.cdr") || lower.ends_with(".cdr"))
            && buf.starts_with(b"RIFF")
        {
            if let Ok(riff) = parse_riff_cdr(buf) {
                if preview.is_none() {
                    preview = riff.preview;
                }
                if embedded_ps_pdf.is_none() {
                    embedded_ps_pdf = riff.embedded_ps_pdf;
                }
                keywords.extend(riff.keywords);
                for (k, v) in riff.metadata {
                    metadata.entry(k).or_insert(v);
                }
            }
        }
    }

    if preview.is_none() {
        if let Some((img, src)) = fallback_preview {
            preview = Some(img);
            metadata.insert("Preview Source".into(), format!("{src} (ZIP local scan)"));
        }
    }
    if preview.is_none() {
        if let Some(img) = scan_embedded_preview_image(bytes) {
            preview = Some(img);
            metadata.insert("Preview Source".into(), "Embedded raw preview".into());
        }
    }

    if preview.is_none() && keywords.is_empty() && embedded_ps_pdf.is_none() {
        return None;
    }

    metadata.insert("Container".into(), "ZIP (Recovered local headers)".into());
    metadata.insert("CDR Version".into(), version_label.clone());
    dedup_keywords(&mut keywords);

    Some(CdrParsed {
        version_label,
        preview,
        keywords,
        metadata,
        embedded_ps_pdf,
        page_count: 1,
    })
}

/// Scans a binary container for an embedded PNG, JPEG, or BMP preview image.
fn scan_embedded_preview_image(bytes: &[u8]) -> Option<DynamicImage> {
    let limit = bytes.len().min(4 * 1024 * 1024);
    let hay = &bytes[..limit];

    // 1. Embedded PNG (\x89PNG\r\n\x1A\n ... IEND)
    let mut pos = 0usize;
    while let Some(rel) = find_subslice(&hay[pos..], b"\x89PNG\r\n\x1A\n") {
        let start = pos + rel;
        let slice = &hay[start..];
        let end = find_subslice(slice, b"IEND")
            .map(|p| (p + 8).min(slice.len()))
            .unwrap_or_else(|| slice.len().min(2 * 1024 * 1024));
        if let Ok(img) = crate::decoder::decode_image_with_limits(&slice[..end], Some(image::ImageFormat::Png)) {
            if img.width() >= 16 && img.height() >= 16 {
                return Some(img);
            }
        }
        pos = start + 8;
    }

    // 2. Embedded BMP ("BM" with plausible file size and DIB header size)
    pos = 0;
    while let Some(rel) = find_subslice(&hay[pos..], b"BM") {
        let start = pos + rel;
        if start + 54 <= hay.len() {
            let fsize = u32::from_le_bytes([
                hay[start + 2],
                hay[start + 3],
                hay[start + 4],
                hay[start + 5],
            ]) as usize;
            let dib_sz = u32::from_le_bytes([
                hay[start + 14],
                hay[start + 15],
                hay[start + 16],
                hay[start + 17],
            ]);
            if matches!(dib_sz, 40 | 52 | 56 | 64 | 108 | 124)
                && fsize >= 54
                && fsize <= 16 * 1024 * 1024
                && start + fsize <= bytes.len()
            {
                if let Some(img) = crate::decoder::decode_bmp_tolerant(&bytes[start..start + fsize]) {
                    if img.width() >= 16 && img.height() >= 16 {
                        return Some(img);
                    }
                }
            }
        }
        pos = start + 2;
    }

    None
}

/// CorelDRAW v2 (WL magic) 1-bit preview decoder
fn parse_wl_v2_cdr(bytes: &[u8]) -> Result<CdrParsed, String> {
    let mut metadata = BTreeMap::new();
    metadata.insert("CDR Version".into(), "CorelDRAW (Version 2)".into());
    metadata.insert("Container".into(), "Legacy WL".into());

    let mut preview = None;
    if bytes.len() > 0x40 {
        let preview_offset =
            u32::from_le_bytes([bytes[0x1c], bytes[0x1d], bytes[0x1e], bytes[0x1f]]) as usize;
        if (0x28..=0xbb7).contains(&preview_offset) && preview_offset + 24 < bytes.len() {
            let w = u32::from_le_bytes([
                bytes[preview_offset + 4],
                bytes[preview_offset + 5],
                bytes[preview_offset + 6],
                bytes[preview_offset + 7],
            ]);
            let h = u32::from_le_bytes([
                bytes[preview_offset + 8],
                bytes[preview_offset + 9],
                bytes[preview_offset + 10],
                bytes[preview_offset + 11],
            ]);
            if w > 0 && w <= 256 && h > 0 && h <= 256 {
                let stride = (((w + 7) / 8) + 3) & !3;
                let data_start = preview_offset + 24;
                if data_start + (stride * h) as usize <= bytes.len() {
                    let mut img = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(w, h);
                    for y in 0..h {
                        let row = &bytes[data_start + ((h - 1 - y) * stride) as usize..];
                        for x in 0..w {
                            let bit = (row[(x / 8) as usize] >> (7 - (x % 8))) & 1;
                            let c = if bit != 0 { 255 } else { 0 };
                            img.put_pixel(x, y, Rgba([c, c, c, 255]));
                        }
                    }
                    preview = Some(DynamicImage::ImageRgba8(img));
                }
            }
        }
    }

    Ok(CdrParsed {
        version_label: "CorelDRAW (Version 2)".into(),
        preview,
        keywords: Vec::new(),
        metadata,
        embedded_ps_pdf: None,
        page_count: 1,
    })
}

fn parse_corel_bmf(bytes: &[u8]) -> Result<CdrParsed, String> {
    let mut metadata = BTreeMap::new();
    metadata.insert("CDR Version".into(), "CorelDRAW (Gallery file)".into());
    metadata.insert("Container".into(), "@CorelBMF".into());

    let mut preview = None;
    let scan_end = bytes.len().min(4096);
    for i in 9..scan_end.saturating_sub(46) {
        if bytes[i] == b'+' {
            if let Some(img) = wrap_dib_as_bmp(&bytes[i + 6..]) {
                preview = Some(img);
                break;
            }
        }
    }

    Ok(CdrParsed {
        version_label: "CorelDRAW (Gallery file)".into(),
        preview,
        keywords: Vec::new(),
        metadata,
        embedded_ps_pdf: None,
        page_count: 1,
    })
}

fn riff_form_to_version(form: &str) -> String {
    match form {
        "CDR3" => "CorelDRAW 3".into(),
        "CDR4" => "CorelDRAW 4".into(),
        "CDR5" => "CorelDRAW 5".into(),
        "CDR6" => "CorelDRAW 6".into(),
        "CDR7" => "CorelDRAW 7".into(),
        "CDR8" => "CorelDRAW 8".into(),
        "CDR9" => "CorelDRAW 9".into(),
        "CDRA" => "CorelDRAW 10".into(),
        "CDRB" => "CorelDRAW 11".into(),
        "CDRC" => "CorelDRAW 12".into(),
        "CDRD" => "CorelDRAW X3 (13)".into(),
        "CDRE" => "CorelDRAW X4 (14)".into(),
        "CDRF" => "CorelDRAW X5 (15)".into(),
        "CDRG" => "CorelDRAW X6 (16)".into(),
        "CDRH" => "CorelDRAW X7 (17)".into(),
        "CDRI" => "CorelDRAW X8 (18)".into(),
        "CDRJ" => "CorelDRAW 2017 (19)".into(),
        "CDRK" => "CorelDRAW 2018 (20)".into(),
        "CDRL" => "CorelDRAW 2019 (21)".into(),
        "CDRM" => "CorelDRAW 2020 (22)".into(),
        "CDRN" => "CorelDRAW 2021 (23)".into(),
        "CDRO" => "CorelDRAW 2022 (24)".into(),
        "CDRP" => "CorelDRAW 2023+ (25)".into(),
        s if s.starts_with("CMX") => format!("Corel Presentation Exchange ({s})"),
        s => format!("CorelDRAW ({s})"),
    }
}

fn decode_riff_text(payload: &[u8]) -> String {
    // Detect UTF-16LE (common in CorelDRAW 12/X3+ summary blocks)
    if payload.len() >= 4 && payload.len() % 2 == 0 {
        let zeros_odd = payload
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|&&b| b == 0)
            .count();
        if zeros_odd > payload.len() / 4 {
            let u16s: Vec<u16> = payload
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&v| v != 0)
                .collect();
            let s = String::from_utf16_lossy(&u16s).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    let nul_pos = payload.iter().position(|&b| b == 0).unwrap_or(payload.len());
    String::from_utf8_lossy(&payload[..nul_pos])
        .trim()
        .to_string()
}

fn extract_summary_chunk_keywords(
    payload: &[u8],
    keywords: &mut Vec<String>,
    metadata: &mut BTreeMap<String, String>,
) {
    // Check if XML/XMP is stored inside udta/sumi
    if let Some((kws, meta)) = extract_embedded_xmp(payload) {
        keywords.extend(kws);
        for (k, v) in meta {
            metadata.entry(k).or_insert(v);
        }
        return;
    }

    // Look for "Keywords:" / "Keywords=" in ASCII or UTF-16LE
    let text = decode_riff_text(payload);
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("keywords:") {
            push_split_keywords(keywords, &line[pos + 9..]);
        } else if let Some(pos) = lower.find("keywords=") {
            push_split_keywords(keywords, &line[pos + 9..]);
        }
    }
}

/// Extracts XMP/RDF/Dublin Core/Corel XML metadata and keywords
pub fn parse_xmp_or_xml_metadata(xml: &str) -> (Vec<String>, BTreeMap<String, String>) {
    let mut keywords = Vec::new();
    let mut meta = BTreeMap::new();

    // Trim any leading/trailing garbage outside XML root
    let trimmed = if let (Some(start), Some(end)) = (xml.find('<'), xml.rfind('>')) {
        if start <= end {
            &xml[start..=end]
        } else {
            xml
        }
    } else {
        xml
    };

    let Ok(doc) = roxmltree::Document::parse(trimmed) else {
        return (keywords, meta);
    };

    for node in doc.descendants().filter(|n| n.is_element()) {
        let tag = node.tag_name().name();
        let tag_lower = tag.to_ascii_lowercase();

        // Check attributes too (some XMP writers store pdf:Keywords="..." or xmp:CreatorTool="..." as attributes on rdf:Description)
        for attr in node.attributes() {
            let a_lower = attr.name().to_ascii_lowercase();
            let val = attr.value().trim();
            if val.is_empty() {
                continue;
            }
            if a_lower == "keywords" || a_lower == "subject" {
                push_split_keywords(&mut keywords, val);
            } else if a_lower == "creatortool" {
                meta.entry("Software".into()).or_insert_with(|| val.into());
            } else if a_lower == "label" {
                meta.entry("Label".into()).or_insert_with(|| val.into());
            }
        }

        // Dublin Core dc:subject, lr:hierarchicalSubject, MicrosoftPhoto:LastKeywordXMP, pdf:Keywords, cp:keywords, Corel Keyword tags
        if tag_lower == "subject"
            || tag_lower == "hierarchicalsubject"
            || tag_lower == "lastkeywordxmp"
            || tag_lower == "keywords"
            || tag_lower == "keyword"
            || tag_lower == "tags"
        {
            let mut found_li = false;
            for child in node.descendants().filter(|c| c.is_element()) {
                if child.tag_name().name().eq_ignore_ascii_case("li") {
                    if let Some(t) = child.text() {
                        found_li = true;
                        push_split_keywords(&mut keywords, t);
                    }
                }
            }
            if !found_li {
                if let Some(t) = node.text() {
                    push_split_keywords(&mut keywords, t);
                }
            }
        } else if tag_lower == "title" {
            if let Some(v) = extract_rdf_or_text(node) {
                meta.entry("Title".into()).or_insert(v);
            }
        } else if tag_lower == "description" && node.tag_name().namespace().map(|ns| ns.contains("purl.org")).unwrap_or(false) {
            if let Some(v) = extract_rdf_or_text(node) {
                meta.entry("Description".into()).or_insert(v);
            }
        } else if tag_lower == "creator" {
            if let Some(v) = extract_rdf_or_text(node) {
                meta.entry("Author".into()).or_insert(v);
            }
        } else if tag_lower == "rights" {
            if let Some(v) = extract_rdf_or_text(node) {
                meta.entry("Copyright".into()).or_insert(v);
            }
        } else if tag_lower == "creatortool" {
            if let Some(t) = node.text() {
                let s = t.trim();
                if !s.is_empty() {
                    meta.entry("Software".into()).or_insert_with(|| s.into());
                }
            }
        } else if tag_lower == "notes" || tag_lower == "comments" {
            if let Some(t) = node.text() {
                let s = t.trim();
                if !s.is_empty() {
                    meta.entry("Comments".into()).or_insert_with(|| s.into());
                }
            }
        }
    }

    dedup_keywords(&mut keywords);
    (keywords, meta)
}

fn extract_rdf_or_text(node: roxmltree::Node) -> Option<String> {
    for child in node.descendants().filter(|c| c.is_element()) {
        if child.tag_name().name().eq_ignore_ascii_case("li") {
            if let Some(t) = child.text() {
                let s = t.trim();
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    node.text()
        .map(|t| t.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Locate and parse an embedded `<?xpacket` or `<x:xmpmeta` XMP block in any binary file
pub fn extract_embedded_xmp(bytes: &[u8]) -> Option<(Vec<String>, BTreeMap<String, String>)> {
    let start = find_subslice(bytes, b"<x:xmpmeta")
        .or_else(|| find_subslice(bytes, b"<rdf:RDF"))?;
    let slice = &bytes[start..bytes.len().min(start + 256 * 1024)];
    let end = find_subslice(slice, b"</x:xmpmeta>")
        .map(|p| p + 12)
        .or_else(|| find_subslice(slice, b"</rdf:RDF>").map(|p| p + 10))?;
    let xml = String::from_utf8_lossy(&slice[..end]);
    let (kws, meta) = parse_xmp_or_xml_metadata(&xml);
    if kws.is_empty() && meta.is_empty() {
        None
    } else {
        Some((kws, meta))
    }
}

pub fn push_split_keywords(out: &mut Vec<String>, raw: &str) {
    for part in raw.split([',', ';', '\n', '\r', '|']) {
        let cleaned = part.trim().trim_matches('"').trim();
        if !cleaned.is_empty() && cleaned.len() <= 128 {
            out.push(cleaned.to_string());
        }
    }
}

pub fn dedup_keywords(kws: &mut Vec<String>) {
    let mut seen = HashSet::new();
    kws.retain(|k| {
        let key = k.to_ascii_lowercase();
        if seen.contains(&key) {
            false
        } else {
            seen.insert(key);
            true
        }
    });
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_template_cdr_prefers_page1_over_degenerate_30000px_strip() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("reference")
            .join("Blank Template.cdr");
        if !path.is_file() {
            return;
        }
        let parsed = parse_cdr(&path).expect("parse Blank Template.cdr");
        let img = parsed.preview.expect("should extract preview from Blank Template.cdr");
        assert!(
            img.width() <= 4096 && img.height() <= 4096,
            "expected sane page preview instead of degenerate 30000x256 strip, got {}x{}",
            img.width(),
            img.height()
        );
        assert_eq!(img.width(), 256);
        assert_eq!(img.height(), 170);
    }
}
