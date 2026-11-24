use crate::cdr;
use flate2::read::ZlibDecoder;
use image::{DynamicImage, ImageBuffer, Rgba};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

#[allow(dead_code)]
#[derive(Debug)]
pub struct PsdParsed {
    pub is_psb: bool,
    pub width: u32,
    pub height: u32,
    pub channels: u16,
    pub depth: u16,
    pub color_mode: u16,
    pub image: Option<DynamicImage>,
    pub keywords: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub source_note: String,
}

/// Parses Adobe Photoshop `.psd` (version 1) and Large Document Format `.psb` (version 2) files.
/// Extracts dimensions, color mode, layer count, XMP/IPTC keywords, embedded JPEG thumbnail
/// (resource 1036/1033), and decodes the merged composite image (Raw, PackBits RLE, or ZIP).
#[allow(dead_code)]
pub fn parse_psd(path: &Path, prefer_fast_thumb: bool) -> Result<PsdParsed, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read PSD/PSB: {e}"))?;
    parse_psd_bytes(&bytes, prefer_fast_thumb)
}

pub fn parse_psd_bytes(bytes: &[u8], prefer_fast_thumb: bool) -> Result<PsdParsed, String> {
    if bytes.len() < 26 || &bytes[0..4] != b"8BPS" {
        return Err("Not a valid Photoshop PSD/PSB file (missing 8BPS signature)".to_string());
    }

    let version = u16::from_be_bytes([bytes[4], bytes[5]]);
    let is_psb = match version {
        1 => false,
        2 => true,
        other => return Err(format!("Unsupported Photoshop version: {other}")),
    };

    let channels = u16::from_be_bytes([bytes[12], bytes[13]]);
    let height = u32::from_be_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    let width = u32::from_be_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let depth = u16::from_be_bytes([bytes[22], bytes[23]]);
    let color_mode = u16::from_be_bytes([bytes[24], bytes[25]]);

    if width == 0 || height == 0 || channels == 0 || channels > 56 {
        return Err(format!(
            "Invalid Photoshop dimensions/channels: {width}x{height} ({channels} ch)"
        ));
    }

    let mut metadata = BTreeMap::new();
    let mut keywords = Vec::new();
    metadata.insert(
        "Format".into(),
        if is_psb {
            "Adobe Photoshop Large Document (PSB v2)".into()
        } else {
            "Adobe Photoshop Document (PSD v1)".into()
        },
    );
    metadata.insert("Color Mode".into(), color_mode_name(color_mode).into());
    metadata.insert("Channels".into(), channels.to_string());
    metadata.insert("Bit Depth".into(), format!("{depth}-bit/channel"));

    let mut pos = 26usize;

    // 1. Color Mode Data section
    if pos + 4 > bytes.len() {
        return Err("Truncated PSD/PSB color mode section".to_string());
    }
    let color_mode_len = read_u32_be(bytes, pos) as usize;
    pos += 4;
    if pos.saturating_add(color_mode_len) > bytes.len() {
        return Err("Truncated PSD/PSB color mode payload".to_string());
    }
    let color_mode_data = &bytes[pos..pos + color_mode_len];
    pos += color_mode_len;

    // 2. Image Resources section (embedded JPEG thumbnail, XMP, IPTC, EXIF)
    let mut embedded_thumb: Option<DynamicImage> = None;
    if pos + 4 <= bytes.len() {
        let res_len = read_u32_be(bytes, pos) as usize;
        pos += 4;
        let res_end = pos.saturating_add(res_len).min(bytes.len());
        parse_image_resources(
            &bytes[pos..res_end],
            &mut embedded_thumb,
            &mut keywords,
            &mut metadata,
        );
        pos = res_end;
    }

    // 3. Layer and Mask Information section (u32 len in PSD, u64 len in PSB)
    let mut has_merged_alpha = false;
    if is_psb {
        if pos + 8 <= bytes.len() {
            let layer_sec_len = read_u64_be(bytes, pos) as usize;
            pos += 8;
            let layer_sec_end = pos.saturating_add(layer_sec_len).min(bytes.len());
            if layer_sec_len >= 10 && pos + 10 <= layer_sec_end {
                let layer_info_len = read_u64_be(bytes, pos);
                if layer_info_len >= 2 {
                    let raw_count = i16::from_be_bytes([bytes[pos + 8], bytes[pos + 9]]);
                    has_merged_alpha = raw_count < 0;
                    metadata.insert("Layers".into(), raw_count.unsigned_abs().to_string());
                }
            }
            pos = layer_sec_end;
        }
    } else if pos + 4 <= bytes.len() {
        let layer_sec_len = read_u32_be(bytes, pos) as usize;
        pos += 4;
        let layer_sec_end = pos.saturating_add(layer_sec_len).min(bytes.len());
        if layer_sec_len >= 6 && pos + 6 <= layer_sec_end {
            let layer_info_len = read_u32_be(bytes, pos);
            if layer_info_len >= 2 {
                let raw_count = i16::from_be_bytes([bytes[pos + 4], bytes[pos + 5]]);
                has_merged_alpha = raw_count < 0;
                metadata.insert("Layers".into(), raw_count.unsigned_abs().to_string());
            }
        }
        pos = layer_sec_end;
    }

    // Also scan for any XMP packet if resource 1060 wasn't found
    if keywords.is_empty() {
        if let Some((xmp_kws, xmp_meta)) = cdr::extract_embedded_xmp(bytes) {
            keywords.extend(xmp_kws);
            for (k, v) in xmp_meta {
                metadata.entry(k).or_insert(v);
            }
        }
    }
    cdr::dedup_keywords(&mut keywords);

    // 4. Merged Composite Image Data section
    let pixels = (width as u64).saturating_mul(height as u64);
    let max_composite_pixels: u64 = if prefer_fast_thumb {
        16_000_000
    } else {
        64_000_000
    };

    let mut image: Option<DynamicImage> = None;
    let mut source_note = if is_psb {
        "PSB Native".to_string()
    } else {
        "PSD Native".to_string()
    };

    // For very large PSD/PSB files in thumbnail mode, prefer the embedded 8BIM JPEG thumbnail if present
    if prefer_fast_thumb && pixels > max_composite_pixels {
        if let Some(thumb) = embedded_thumb.take() {
            image = Some(thumb);
            source_note = if is_psb {
                "PSB Embedded Thumbnail".to_string()
            } else {
                "PSD Embedded Thumbnail".to_string()
            };
        }
    }

    if image.is_none() && pos + 2 <= bytes.len() && pixels <= max_composite_pixels {
        match decode_composite_image(
            &bytes[pos..],
            is_psb,
            width,
            height,
            channels,
            depth,
            color_mode,
            color_mode_data,
            has_merged_alpha,
        ) {
            Ok(img) => {
                image = Some(img);
                source_note = if is_psb {
                    "PSB Composite".to_string()
                } else {
                    "PSD Composite".to_string()
                };
            }
            Err(err) => {
                metadata.insert("Composite Note".into(), err);
            }
        }
    }

    if image.is_none() {
        if let Some(thumb) = embedded_thumb {
            image = Some(thumb);
            source_note = if is_psb {
                "PSB Embedded Thumbnail".to_string()
            } else {
                "PSD Embedded Thumbnail".to_string()
            };
        }
    }

    Ok(PsdParsed {
        is_psb,
        width,
        height,
        channels,
        depth,
        color_mode,
        image,
        keywords,
        metadata,
        source_note,
    })
}

fn color_mode_name(mode: u16) -> &'static str {
    match mode {
        0 => "Bitmap",
        1 => "Grayscale",
        2 => "Indexed",
        3 => "RGB",
        4 => "CMYK",
        7 => "Multichannel",
        8 => "Duotone",
        9 => "Lab",
        _ => "Unknown",
    }
}

fn parse_image_resources(
    data: &[u8],
    embedded_thumb: &mut Option<DynamicImage>,
    keywords: &mut Vec<String>,
    metadata: &mut BTreeMap<String, String>,
) {
    let mut pos = 0usize;
    while pos + 12 <= data.len() {
        let sig = &data[pos..pos + 4];
        if sig != b"8BIM" && sig != b"MeSa" && sig != b"AgHg" && sig != b"PHUT" && sig != b"DCSR" {
            break;
        }
        let res_id = u16::from_be_bytes([data[pos + 4], data[pos + 5]]);
        pos += 6;

        // Pascal string name padded to even total length (length byte + name bytes)
        if pos >= data.len() {
            break;
        }
        let name_len = data[pos] as usize;
        let pascal_total = 1 + name_len;
        let padded_pascal = pascal_total + (pascal_total & 1);
        pos = pos.saturating_add(padded_pascal);
        if pos + 4 > data.len() {
            break;
        }

        let block_size = read_u32_be(data, pos) as usize;
        pos += 4;
        let block_end = pos.saturating_add(block_size).min(data.len());
        let block = &data[pos..block_end];

        match res_id {
            // 1036 = Photoshop 5.0+ RGB JPEG thumbnail; 1033 = Photoshop 4.0 BGR JPEG thumbnail
            1036 | 1033 => {
                if embedded_thumb.is_none() || res_id == 1036 {
                    if let Some(img) = decode_thumbnail_resource(block, res_id == 1033) {
                        *embedded_thumb = Some(img);
                    }
                }
            }
            // 1060 = XMP metadata XML
            1060 => {
                let xml = String::from_utf8_lossy(block);
                let (kws, meta) = cdr::parse_xmp_or_xml_metadata(&xml);
                keywords.extend(kws);
                for (k, v) in meta {
                    metadata.entry(k).or_insert(v);
                }
            }
            // 1028 = IPTC-NAA record
            1028 => {
                parse_iptc_block(block, keywords, metadata);
            }
            _ => {}
        }

        let padded_size = block_size.saturating_add(block_size & 1);
        let next = pos.saturating_add(padded_size);
        if next <= pos {
            break;
        }
        pos = next;
    }
}

fn decode_thumbnail_resource(block: &[u8], swap_rb: bool) -> Option<DynamicImage> {
    // 28-byte header: format(4), width(4), height(4), widthbytes(4), totalsize(4), compressedsize(4), bpp(2), planes(2)
    if block.len() <= 28 {
        return None;
    }
    let fmt = read_u32_be(block, 0);
    if fmt != 1 {
        // 1 = kJpegRGB; 0 = raw RGB (rare)
        return None;
    }
    let jpeg = &block[28..];
    let mut img = image::load_from_memory(jpeg).ok()?;
    if swap_rb {
        let mut rgba = img.to_rgba8();
        for px in rgba.pixels_mut() {
            px.0.swap(0, 2);
        }
        img = DynamicImage::ImageRgba8(rgba);
    }
    Some(img)
}

fn parse_iptc_block(
    data: &[u8],
    keywords: &mut Vec<String>,
    metadata: &mut BTreeMap<String, String>,
) {
    let mut pos = 0usize;
    while pos + 5 <= data.len() {
        if data[pos] != 0x1C {
            pos += 1;
            continue;
        }
        let record = data[pos + 1];
        let dataset = data[pos + 2];
        let size_raw = u16::from_be_bytes([data[pos + 3], data[pos + 4]]) as usize;
        pos += 5;
        if (size_raw & 0x8000) != 0 {
            // Extended dataset tag
            let ext_len_bytes = size_raw & 0x7FFF;
            if pos + ext_len_bytes > data.len() {
                break;
            }
            let mut ext_size = 0usize;
            for &b in &data[pos..pos + ext_len_bytes] {
                ext_size = (ext_size << 8) | (b as usize);
            }
            pos += ext_len_bytes;
            pos = pos.saturating_add(ext_size);
            continue;
        }
        let end = pos.saturating_add(size_raw).min(data.len());
        let val = String::from_utf8_lossy(&data[pos..end]).trim().to_string();
        if record == 2 && !val.is_empty() {
            match dataset {
                25 => cdr::push_split_keywords(keywords, &val),
                5 => {
                    metadata.entry("Title".into()).or_insert(val);
                }
                80 => {
                    metadata.entry("Author".into()).or_insert(val);
                }
                116 => {
                    metadata.entry("Copyright".into()).or_insert(val);
                }
                120 => {
                    metadata.entry("Description".into()).or_insert(val);
                }
                _ => {}
            }
        }
        pos = end;
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_composite_image(
    data: &[u8],
    is_psb: bool,
    width: u32,
    height: u32,
    channels: u16,
    depth: u16,
    color_mode: u16,
    color_mode_data: &[u8],
    has_merged_alpha: bool,
) -> Result<DynamicImage, String> {
    if data.len() < 2 {
        return Err("Missing composite image compression header".to_string());
    }
    let compression = u16::from_be_bytes([data[0], data[1]]);
    let payload = &data[2..];

    let needed_channels = match color_mode {
        0 | 2 => 1usize,                                      // Bitmap / Indexed
        1 | 8 => if has_merged_alpha && channels >= 2 { 2 } else { 1 }, // Grayscale / Duotone
        3 | 9 => if has_merged_alpha && channels >= 4 { 4 } else { 3 }, // RGB / Lab
        4 => if has_merged_alpha && channels >= 5 { 5 } else { 4 },     // CMYK
        _ => (channels as usize).min(4),
    };
    let read_channels = needed_channels.min(channels as usize);
    let bytes_per_row = match depth {
        1 => (width as usize).div_ceil(8),
        8 => width as usize,
        16 => (width as usize).saturating_mul(2),
        32 => (width as usize).saturating_mul(4),
        other => return Err(format!("Unsupported PSD/PSB bit depth: {other}")),
    };
    let plane_bytes = bytes_per_row.saturating_mul(height as usize);
    let total_planes_bytes = plane_bytes.saturating_mul(read_channels);

    let planes: Vec<Vec<u8>> = match compression {
        // 0: Raw uncompressed planar data
        0 => {
            if payload.len() < total_planes_bytes {
                return Err("Truncated raw PSD/PSB composite image".to_string());
            }
            (0..read_channels)
                .map(|c| payload[c * plane_bytes..(c + 1) * plane_bytes].to_vec())
                .collect()
        }
        // 1: PackBits RLE planar data (u16 row counts in PSD, u32 row counts in PSB)
        1 => decode_packbits_planes(
            payload,
            is_psb,
            channels as usize,
            read_channels,
            height as usize,
            bytes_per_row,
        )?,
        // 2 / 3: ZIP compressed composite
        2 | 3 => {
            let mut dec = ZlibDecoder::new(payload);
            let mut raw = Vec::with_capacity(total_planes_bytes);
            dec.read_to_end(&mut raw)
                .map_err(|e| format!("PSD/PSB ZIP decode error: {e}"))?;
            if raw.len() < total_planes_bytes {
                return Err("Truncated ZIP PSD/PSB composite image".to_string());
            }
            let mut out_planes: Vec<Vec<u8>> = (0..read_channels)
                .map(|c| raw[c * plane_bytes..(c + 1) * plane_bytes].to_vec())
                .collect();
            if compression == 3 {
                for plane in &mut out_planes {
                    unpredict_horizontal(plane, width as usize, height as usize, depth);
                }
            }
            out_planes
        }
        other => return Err(format!("Unsupported PSD/PSB compression: {other}")),
    };

    // Convert planar samples to 8-bit values per channel
    let pixel_count = (width as usize).saturating_mul(height as usize);
    let mut ch8: Vec<Vec<u8>> = Vec::with_capacity(read_channels);
    for plane in &planes {
        ch8.push(plane_to_u8(plane, width as usize, height as usize, depth));
    }

    let mut rgba = vec![255u8; pixel_count * 4];
    match color_mode {
        // Bitmap (0 = white, 1 = black in Photoshop Bitmap mode)
        0 => {
            let c0 = &ch8[0];
            for i in 0..pixel_count {
                let v = 255u8.saturating_sub(c0[i]);
                let o = i * 4;
                rgba[o] = v;
                rgba[o + 1] = v;
                rgba[o + 2] = v;
                rgba[o + 3] = 255;
            }
        }
        // Grayscale / Duotone
        1 | 8 => {
            let g = &ch8[0];
            let a = ch8.get(1);
            for i in 0..pixel_count {
                let v = g[i];
                let o = i * 4;
                rgba[o] = v;
                rgba[o + 1] = v;
                rgba[o + 2] = v;
                rgba[o + 3] = a.map_or(255, |al| al[i]);
            }
        }
        // Indexed (768-byte planar R[256], G[256], B[256] palette)
        2 => {
            let idx = &ch8[0];
            let has_pal = color_mode_data.len() >= 768;
            for i in 0..pixel_count {
                let k = idx[i] as usize;
                let o = i * 4;
                if has_pal {
                    rgba[o] = color_mode_data[k];
                    rgba[o + 1] = color_mode_data[256 + k];
                    rgba[o + 2] = color_mode_data[512 + k];
                } else {
                    rgba[o] = idx[i];
                    rgba[o + 1] = idx[i];
                    rgba[o + 2] = idx[i];
                }
                rgba[o + 3] = 255;
            }
        }
        // RGB
        3 if ch8.len() >= 3 => {
            let r = &ch8[0];
            let g = &ch8[1];
            let b = &ch8[2];
            let a = ch8.get(3);
            for i in 0..pixel_count {
                let o = i * 4;
                rgba[o] = r[i];
                rgba[o + 1] = g[i];
                rgba[o + 2] = b[i];
                rgba[o + 3] = a.map_or(255, |al| al[i]);
            }
        }
        // CMYK (Photoshop stores inverted CMYK where 255 = 0% ink, 0 = 100% ink)
        4 if ch8.len() >= 4 => {
            let c = &ch8[0];
            let m = &ch8[1];
            let y = &ch8[2];
            let k = &ch8[3];
            let a = ch8.get(4);
            for i in 0..pixel_count {
                let kv = k[i] as u16;
                let o = i * 4;
                rgba[o] = ((c[i] as u16 * kv) / 255) as u8;
                rgba[o + 1] = ((m[i] as u16 * kv) / 255) as u8;
                rgba[o + 2] = ((y[i] as u16 * kv) / 255) as u8;
                rgba[o + 3] = a.map_or(255, |al| al[i]);
            }
        }
        // CIE Lab
        9 if ch8.len() >= 3 => {
            let l_ch = &ch8[0];
            let a_ch = &ch8[1];
            let b_ch = &ch8[2];
            let alpha = ch8.get(3);
            for i in 0..pixel_count {
                let (r, g, b) = lab_u8_to_srgb(l_ch[i], a_ch[i], b_ch[i]);
                let o = i * 4;
                rgba[o] = r;
                rgba[o + 1] = g;
                rgba[o + 2] = b;
                rgba[o + 3] = alpha.map_or(255, |al| al[i]);
            }
        }
        // Multichannel / fallback
        _ => {
            let r = &ch8[0];
            let g = ch8.get(1).unwrap_or(r);
            let b = ch8.get(2).unwrap_or(r);
            for i in 0..pixel_count {
                let o = i * 4;
                rgba[o] = r[i];
                rgba[o + 1] = g[i];
                rgba[o + 2] = b[i];
                rgba[o + 3] = 255;
            }
        }
    }

    ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba)
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| "Failed assembling PSD/PSB RGBA buffer".to_string())
}

fn decode_packbits_planes(
    payload: &[u8],
    is_psb: bool,
    total_channels: usize,
    read_channels: usize,
    height: usize,
    bytes_per_row: usize,
) -> Result<Vec<Vec<u8>>, String> {
    let total_rows = total_channels.saturating_mul(height);
    let count_width = if is_psb { 4usize } else { 2usize };
    let table_bytes = total_rows.saturating_mul(count_width);
    if table_bytes > payload.len() {
        return Err("Truncated PSD/PSB RLE scanline byte-count table".to_string());
    }

    let mut row_lengths = Vec::with_capacity(read_channels.saturating_mul(height));
    for r in 0..read_channels.saturating_mul(height) {
        let off = r * count_width;
        let len = if is_psb {
            read_u32_be(payload, off) as usize
        } else {
            u16::from_be_bytes([payload[off], payload[off + 1]]) as usize
        };
        row_lengths.push(len);
    }

    let mut src_pos = table_bytes;
    let mut planes = Vec::with_capacity(read_channels);
    for ch in 0..read_channels {
        let mut plane = Vec::with_capacity(bytes_per_row.saturating_mul(height));
        for y in 0..height {
            let rlen = row_lengths[ch * height + y];
            let row_end = src_pos.saturating_add(rlen).min(payload.len());
            let row_src = &payload[src_pos..row_end];
            src_pos = row_end;

            let row_start_out = plane.len();
            decode_packbits_row(row_src, &mut plane, bytes_per_row);
            let produced = plane.len() - row_start_out;
            if produced < bytes_per_row {
                plane.resize(row_start_out + bytes_per_row, 0);
            } else if produced > bytes_per_row {
                plane.truncate(row_start_out + bytes_per_row);
            }
        }
        planes.push(plane);
    }
    Ok(planes)
}

fn decode_packbits_row(src: &[u8], dst: &mut Vec<u8>, max_out: usize) {
    let start_len = dst.len();
    let mut i = 0usize;
    while i < src.len() && (dst.len() - start_len) < max_out {
        let header = src[i] as i8;
        i += 1;
        if header >= 0 {
            let count = (header as usize) + 1;
            let avail = count.min(src.len() - i).min(max_out - (dst.len() - start_len));
            dst.extend_from_slice(&src[i..i + avail]);
            i += count.min(src.len() - i);
        } else if header != -128 {
            let count = (1 - (header as i32)) as usize;
            if i >= src.len() {
                break;
            }
            let val = src[i];
            i += 1;
            let take = count.min(max_out - (dst.len() - start_len));
            dst.resize(dst.len() + take, val);
        }
    }
}

fn unpredict_horizontal(plane: &mut [u8], width: usize, height: usize, depth: u16) {
    match depth {
        8 => {
            for y in 0..height {
                let row = &mut plane[y * width..(y + 1) * width];
                for x in 1..width {
                    row[x] = row[x].wrapping_add(row[x - 1]);
                }
            }
        }
        16 => {
            let stride = width * 2;
            for y in 0..height {
                let row = &mut plane[y * stride..(y + 1) * stride];
                for x in 1..width {
                    let prev = u16::from_be_bytes([row[(x - 1) * 2], row[(x - 1) * 2 + 1]]);
                    let cur = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]);
                    let sum = prev.wrapping_add(cur).to_be_bytes();
                    row[x * 2] = sum[0];
                    row[x * 2 + 1] = sum[1];
                }
            }
        }
        _ => {}
    }
}

fn plane_to_u8(plane: &[u8], width: usize, height: usize, depth: u16) -> Vec<u8> {
    let pixels = width.saturating_mul(height);
    match depth {
        1 => {
            let stride = width.div_ceil(8);
            let mut out = Vec::with_capacity(pixels);
            for y in 0..height {
                let row = &plane[y * stride..];
                for x in 0..width {
                    let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
                    out.push(if bit != 0 { 255 } else { 0 });
                }
            }
            out
        }
        8 => {
            let mut out = plane[..pixels.min(plane.len())].to_vec();
            out.resize(pixels, 0);
            out
        }
        16 => {
            let mut out = Vec::with_capacity(pixels);
            for chunk in plane.chunks_exact(2).take(pixels) {
                out.push(chunk[0]); // High byte of big-endian u16
            }
            out.resize(pixels, 0);
            out
        }
        32 => {
            let mut out = Vec::with_capacity(pixels);
            for chunk in plane.chunks_exact(4).take(pixels) {
                let f = f32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let mapped = if f.is_finite() {
                    (f.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8
                } else {
                    0
                };
                out.push(mapped);
            }
            out.resize(pixels, 0);
            out
        }
        _ => vec![0; pixels],
    }
}

fn lab_u8_to_srgb(l_u8: u8, a_u8: u8, b_u8: u8) -> (u8, u8, u8) {
    let l = (l_u8 as f32) * (100.0 / 255.0);
    let a = (a_u8 as f32) - 128.0;
    let b = (b_u8 as f32) - 128.0;

    let fy = (l + 16.0) / 116.0;
    let fx = a / 500.0 + fy;
    let fz = fy - b / 200.0;

    let inv = |t: f32| {
        let t3 = t * t * t;
        if t3 > 0.008_856 {
            t3
        } else {
            (t - 16.0 / 116.0) / 7.787
        }
    };
    let x = 0.95047 * inv(fx);
    let y = 1.00000 * inv(fy);
    let z = 1.08883 * inv(fz);

    let r_lin = 3.2406 * x - 1.5372 * y - 0.4986 * z;
    let g_lin = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let b_lin = 0.0557 * x - 0.2040 * y + 1.0570 * z;

    let gamma = |c: f32| -> u8 {
        let c = c.clamp(0.0, 1.0);
        let s = if c <= 0.003_130_8 {
            12.92 * c
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round().clamp(0.0, 255.0) as u8
    };
    (gamma(r_lin), gamma(g_lin), gamma(b_lin))
}

fn read_u32_be(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn read_u64_be(b: &[u8], off: usize) -> u64 {
    u64::from_be_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_synthetic_psd_or_psb(is_psb: bool, rle: bool) -> Vec<u8> {
        let w = 8u32;
        let h = 6u32;
        let channels = 3u16;
        let mut out = Vec::new();
        // Header (26 bytes)
        out.extend_from_slice(b"8BPS");
        out.extend_from_slice(&(if is_psb { 2u16 } else { 1u16 }).to_be_bytes());
        out.extend_from_slice(&[0u8; 6]);
        out.extend_from_slice(&channels.to_be_bytes());
        out.extend_from_slice(&h.to_be_bytes());
        out.extend_from_slice(&w.to_be_bytes());
        out.extend_from_slice(&8u16.to_be_bytes()); // 8-bit
        out.extend_from_slice(&3u16.to_be_bytes()); // RGB

        // Color Mode Data (empty)
        out.extend_from_slice(&0u32.to_be_bytes());

        // Image Resources: Resource 1060 (XMP with keywords)
        let xmp = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:dc="http://purl.org/dc/elements/1.1/"><rdf:Description rdf:about=""><dc:subject><rdf:Bag><rdf:li>photoshop</rdf:li><rdf:li>layered</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let mut res = Vec::new();
        res.extend_from_slice(b"8BIM");
        res.extend_from_slice(&1060u16.to_be_bytes());
        res.extend_from_slice(&[0u8, 0u8]); // empty Pascal string padded to 2 bytes
        res.extend_from_slice(&(xmp.len() as u32).to_be_bytes());
        res.extend_from_slice(xmp);
        if xmp.len() % 2 == 1 {
            res.push(0);
        }
        out.extend_from_slice(&(res.len() as u32).to_be_bytes());
        out.extend_from_slice(&res);

        // Layer and Mask Info (empty: 4 bytes for PSD, 8 bytes for PSB)
        if is_psb {
            out.extend_from_slice(&0u64.to_be_bytes());
        } else {
            out.extend_from_slice(&0u32.to_be_bytes());
        }

        // Image Data section: solid R=200, G=100, B=50
        let vals = [200u8, 100u8, 50u8];
        if rle {
            out.extend_from_slice(&1u16.to_be_bytes()); // PackBits RLE
            // Each 8-byte identical run encodes as 2 bytes: [-7 (0xF9), val]
            for _ in 0..(channels as usize * h as usize) {
                if is_psb {
                    out.extend_from_slice(&2u32.to_be_bytes());
                } else {
                    out.extend_from_slice(&2u16.to_be_bytes());
                }
            }
            for v in vals {
                for _ in 0..h {
                    out.push((-7i8) as u8);
                    out.push(v);
                }
            }
        } else {
            out.extend_from_slice(&0u16.to_be_bytes()); // Raw
            for v in vals {
                out.extend(std::iter::repeat_n(v, (w * h) as usize));
            }
        }
        out
    }

    #[test]
    fn decode_psd_raw_and_psb_rle_with_xmp_keywords() {
        for (is_psb, rle) in [(false, false), (false, true), (true, false), (true, true)] {
            let bytes = build_synthetic_psd_or_psb(is_psb, rle);
            let parsed = parse_psd_bytes(&bytes, false).unwrap();
            assert_eq!(parsed.is_psb, is_psb);
            assert_eq!(parsed.width, 8);
            assert_eq!(parsed.height, 6);
            assert!(parsed.keywords.iter().any(|k| k == "photoshop"));
            assert!(parsed.keywords.iter().any(|k| k == "layered"));
            let img = parsed.image.expect("expected decoded composite image");
            let px = img.to_rgba8().get_pixel(3, 2).0;
            assert_eq!(px, [200, 100, 50, 255]);
        }
    }
}
