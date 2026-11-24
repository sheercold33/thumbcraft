use image::{DynamicImage, ImageBuffer, Rgba};
use std::collections::BTreeMap;

const CPT9_MAGIC: &[u8; 8] = b"CPT9FILE";
const CPT8_MAGIC: &[u8; 8] = b"CPT8FILE";
const CPT7_MAGIC: &[u8; 8] = b"CPT7FILE";

const MAX_CPT_DIM: u32 = 32_768;
const MAX_CPT_PIXELS: u64 = 128_000_000;

#[derive(Debug, Clone)]
pub struct CptHeaderInfo {
    pub version_label: String,
    pub width: u32,
    pub height: u32,
    pub color_depth: u16,
    pub dpi_x: Option<u32>,
    pub dpi_y: Option<u32>,
}

#[derive(Debug)]
pub struct CptParsed {
    pub info: CptHeaderInfo,
    pub image: Option<DynamicImage>,
    pub metadata: BTreeMap<String, String>,
    pub source_note: String,
}

#[derive(Debug, Clone, Copy)]
struct CptBlock {
    width: u32,
    height: u32,
    tile_w: u32,
    tile_h: u32,
    bpp: u32,
    block_type: u32,
    tile_table_off: usize,
}

#[inline]
fn read_u16_le(bytes: &[u8], off: usize) -> Option<u16> {
    let s = bytes.get(off..off + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

#[inline]
fn read_u32_le(bytes: &[u8], off: usize) -> Option<u32> {
    let s = bytes.get(off..off + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline]
fn read_u64_le(bytes: &[u8], off: usize) -> Option<u64> {
    let s = bytes.get(off..off + 8)?;
    Some(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

pub fn is_cpt_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 8
        && (&bytes[0..8] == CPT9_MAGIC
            || &bytes[0..8] == CPT8_MAGIC
            || &bytes[0..8] == CPT7_MAGIC)
}

fn parse_cpt_blocks(bytes: &[u8]) -> Result<(CptHeaderInfo, Vec<CptBlock>, Option<usize>), String> {
    if bytes.len() < 0x40 {
        return Err("CPT file too small".into());
    }
    let (version_label, is_64bit_table) = if &bytes[0..8] == CPT9_MAGIC {
        ("Corel PHOTO-PAINT 9+".to_string(), true)
    } else if &bytes[0..8] == CPT8_MAGIC {
        ("Corel PHOTO-PAINT 8".to_string(), false)
    } else if &bytes[0..8] == CPT7_MAGIC {
        ("Corel PHOTO-PAINT 7".to_string(), false)
    } else {
        return Err("Unrecognized CPT header signature".into());
    };

    // Resolution at 0x18 / 0x1C stored in 1000 * pixels-per-meter (e.g. 2834646 = 72 DPI)
    let raw_rx = read_u32_le(bytes, 0x18).unwrap_or(0);
    let raw_ry = read_u32_le(bytes, 0x1C).unwrap_or(0);
    let to_dpi = |v: u32| -> Option<u32> {
        if v < 10_000 {
            return None;
        }
        let d = (v as f64 / 39_370.07874).round() as u32;
        if (10..=10_000).contains(&d) {
            Some(d)
        } else {
            None
        }
    };
    let dpi_x = to_dpi(raw_rx);
    let dpi_y = to_dpi(raw_ry);

    let block_count = read_u32_le(bytes, 0x28).unwrap_or(0).min(1024) as usize;
    let table_off = read_u32_le(bytes, 0x34).unwrap_or(0) as usize;
    let meta_off = read_u32_le(bytes, 0x38)
        .map(|v| v as usize)
        .filter(|&v| v > 0 && v < bytes.len());

    let mut blocks = Vec::new();
    if block_count > 0 && table_off > 0 && table_off < bytes.len() {
        let entry_sz = if is_64bit_table { 8 } else { 4 };
        for i in 0..block_count {
            let off_pos = table_off + i * entry_sz;
            let blk_off = if is_64bit_table {
                read_u64_le(bytes, off_pos).unwrap_or(0) as usize
            } else {
                read_u32_le(bytes, off_pos).unwrap_or(0) as usize
            };
            if blk_off == 0 || blk_off + 0x44 > bytes.len() {
                continue;
            }
            let width = read_u32_le(bytes, blk_off).unwrap_or(0);
            let height = read_u32_le(bytes, blk_off + 4).unwrap_or(0);
            let tile_w = read_u32_le(bytes, blk_off + 8).unwrap_or(0);
            let tile_h = read_u32_le(bytes, blk_off + 12).unwrap_or(0);
            let bpp = read_u32_le(bytes, blk_off + 16).unwrap_or(0);
            let block_type = read_u32_le(bytes, blk_off + 24).unwrap_or(0);
            let meta_skip = read_u32_le(bytes, blk_off + 0x3C).unwrap_or(0) as usize;
            let tile_table_off = blk_off.saturating_add(0x3C).saturating_add(meta_skip);

            if width > 0
                && height > 0
                && width <= MAX_CPT_DIM
                && height <= MAX_CPT_DIM
                && (width as u64) * (height as u64) <= MAX_CPT_PIXELS
            {
                blocks.push(CptBlock {
                    width,
                    height,
                    tile_w,
                    tile_h,
                    bpp,
                    block_type,
                    tile_table_off,
                });
            }
        }
    }

    // Canvas dimensions come from the main image block (type 0x08) or header block (type 0x01)
    let primary = blocks
        .iter()
        .find(|b| b.block_type == 0x08)
        .or_else(|| blocks.iter().find(|b| b.block_type == 0x01))
        .or_else(|| blocks.first());

    let (width, height, color_depth) = match primary {
        Some(b) => (b.width, b.height, b.bpp.clamp(1, 64) as u16),
        None => (0, 0, 24),
    };

    Ok((
        CptHeaderInfo {
            version_label,
            width,
            height,
            color_depth,
            dpi_x,
            dpi_y,
        },
        blocks,
        meta_off,
    ))
}

/// Fast header-only inspection for Info pane metadata.
pub fn inspect_cpt_header(bytes: &[u8]) -> Option<CptHeaderInfo> {
    parse_cpt_blocks(bytes).ok().map(|(info, _, _)| info)
}

/// Decodes a Corel PHOTO-PAINT `.cpt` file.
/// When `prefer_fast_thumb` is true, prefers the embedded `0x10` thumbnail block (fast, 1–4 tiles);
/// otherwise decodes the full-resolution `0x08` image block (compositing the `0x04` alpha mask if present).
pub fn parse_cpt_bytes(bytes: &[u8], prefer_fast_thumb: bool) -> Result<CptParsed, String> {
    let (info, blocks, meta_off) = parse_cpt_blocks(bytes)?;
    let mut metadata = BTreeMap::new();
    metadata.insert("CPT Version".into(), info.version_label.clone());
    if let (Some(dx), Some(dy)) = (info.dpi_x, info.dpi_y) {
        let dpi_str = if dx == dy {
            format!("{dx} DPI")
        } else {
            format!("{dx} × {dy} DPI")
        };
        metadata.insert(crate::image_meta::KEY_DPI.into(), dpi_str);
    }

    // Extract optional META block info (color model, profile, UTF-16LE author/note)
    if let Some(moff) = meta_off {
        parse_cpt_meta_tail(&bytes[moff..], &mut metadata);
    }

    let thumb_blk = blocks.iter().find(|b| b.block_type == 0x10);
    let main_blk = blocks.iter().find(|b| b.block_type == 0x08);
    let mask_blk = blocks.iter().find(|b| b.block_type == 0x04);

    let mut image: Option<DynamicImage> = None;
    let mut source_note = "CPT (No Image)".to_string();

    if prefer_fast_thumb {
        if let Some(tb) = thumb_blk {
            if let Some(img) = decode_cpt_block(bytes, tb, None) {
                image = Some(img);
                source_note = format!("CPT9 Preview ({}×{})", tb.width, tb.height);
            }
        }
        if image.is_none() {
            if let Some(mb) = main_blk {
                let matching_mask = mask_blk.filter(|m| m.width == mb.width && m.height == mb.height);
                if let Some(img) = decode_cpt_block(bytes, mb, matching_mask) {
                    image = Some(img);
                    source_note = format!("CPT9 Image ({}×{})", mb.width, mb.height);
                }
            }
        }
    } else {
        if let Some(mb) = main_blk {
            let matching_mask = mask_blk.filter(|m| m.width == mb.width && m.height == mb.height);
            if let Some(img) = decode_cpt_block(bytes, mb, matching_mask) {
                image = Some(img);
                source_note = format!("CPT9 Full ({}×{})", mb.width, mb.height);
            }
        }
        if image.is_none() {
            if let Some(tb) = thumb_blk {
                if let Some(img) = decode_cpt_block(bytes, tb, None) {
                    image = Some(img);
                    source_note = format!("CPT9 Preview ({}×{})", tb.width, tb.height);
                }
            }
        }
    }

    Ok(CptParsed {
        info,
        image,
        metadata,
        source_note,
    })
}

fn parse_cpt_meta_tail(tail: &[u8], metadata: &mut BTreeMap<String, String>) {
    // Decode UTF-16LE XML color context if present (`<ColorModel>Rgb</ColorModel>`)
    if tail.len() >= 16 && tail.len() <= 256 * 1024 {
        let mut ascii_like = String::with_capacity(tail.len() / 2);
        for chunk in tail.chunks_exact(2) {
            let u = u16::from_le_bytes([chunk[0], chunk[1]]);
            if (0x20..=0x7E).contains(&u) {
                ascii_like.push(u as u8 as char);
            } else {
                ascii_like.push(' ');
            }
        }
        if let Some(start) = ascii_like.find("<ColorModel>") {
            let rest = &ascii_like[start + 12..];
            if let Some(end) = rest.find("</ColorModel>") {
                let model = rest[..end].trim();
                if !model.is_empty() && model.len() <= 32 {
                    metadata.insert("Color Model".into(), model.to_string());
                }
            }
        }
    }
}

fn decode_cpt_block(
    bytes: &[u8],
    blk: &CptBlock,
    alpha_mask_blk: Option<&CptBlock>,
) -> Option<DynamicImage> {
    let w = blk.width as usize;
    let h = blk.height as usize;
    let tw = blk.tile_w.max(1) as usize;
    let th = blk.tile_h.max(1) as usize;

    let bytes_per_px: usize = match blk.bpp {
        8 => 1,
        24 => 3,
        32 => 4,
        _ => return None,
    };

    let nx = w.div_ceil(tw);
    let ny = h.div_ceil(th);
    let num_tiles = nx.checked_mul(ny)?;
    if num_tiles == 0 || num_tiles > 65_536 {
        return None;
    }

    // Verify tile table fits
    let table_end = blk.tile_table_off.checked_add(num_tiles.checked_mul(8)?)?;
    if table_end > bytes.len() {
        return None;
    }

    // Optional alpha mask plane (8-bit, bottom-up just like the image block)
    let alpha_plane = alpha_mask_blk.and_then(|mb| decode_cpt_gray_plane(bytes, mb));

    let mut rgba = vec![255u8; w * h * 4];
    let tile_stride = (tw * bytes_per_px + 3) & !3;

    let mut decoded_any = false;
    for ty in 0..ny {
        for tx in 0..nx {
            let tidx = ty * nx + tx;
            let entry_off = blk.tile_table_off + tidx * 8;
            let tile_off = read_u32_le(bytes, entry_off)? as usize;
            let tile_sz = read_u32_le(bytes, entry_off + 4)? as usize;
            if tile_off == 0 || tile_sz == 0 || tile_off.saturating_add(tile_sz) > bytes.len() {
                continue;
            }
            let tile_data = &bytes[tile_off..tile_off + tile_sz];
            let cw = tw.min(w - tx * tw);
            let ch = th.min(h - ty * th);
            let cropped_stride = (cw * bytes_per_px + 3) & !3;

            let expected_full = tile_stride.saturating_mul(th);
            let raw_buf = decode_cpt9_tile(tile_data, tile_stride, cropped_stride, ch, bytes_per_px, expected_full)?;
            let stride = if raw_buf.len() >= tile_stride * ch {
                tile_stride
            } else if raw_buf.len() >= cropped_stride * ch {
                cropped_stride
            } else {
                continue;
            };
            decoded_any = true;

            for y in 0..ch {
                // Corel CPT stores tiles bottom-up (Windows DIB convention)
                let src_global_y = ty * th + y;
                if src_global_y >= h {
                    break;
                }
                let dst_y = h - 1 - src_global_y;
                let row_off = y * stride;
                for x in 0..cw {
                    let dst_x = tx * tw + x;
                    let s = row_off + x * bytes_per_px;
                    let d = (dst_y * w + dst_x) * 4;
                    match bytes_per_px {
                        1 => {
                            let v = raw_buf[s];
                            rgba[d] = v;
                            rgba[d + 1] = v;
                            rgba[d + 2] = v;
                            rgba[d + 3] = 255;
                        }
                        3 => {
                            // BGR -> RGB
                            rgba[d] = raw_buf[s + 2];
                            rgba[d + 1] = raw_buf[s + 1];
                            rgba[d + 2] = raw_buf[s];
                            rgba[d + 3] = 255;
                        }
                        4 => {
                            // BGRA or CMYK: treat 32-bit as BGRA (standard CPT 32-bit RGB+A)
                            rgba[d] = raw_buf[s + 2];
                            rgba[d + 1] = raw_buf[s + 1];
                            rgba[d + 2] = raw_buf[s];
                            rgba[d + 3] = raw_buf[s + 3];
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    if !decoded_any {
        return None;
    }

    if let Some(mask) = alpha_plane {
        if mask.len() == w * h {
            // Only apply mask if it actually contains meaningful non-zero pixels
            let has_opaque = mask.iter().any(|&a| a > 0);
            let has_transp = mask.iter().any(|&a| a < 255);
            if has_opaque && has_transp {
                for i in 0..w * h {
                    rgba[i * 4 + 3] = mask[i];
                }
            }
        }
    }

    ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(blk.width, blk.height, rgba)
        .map(DynamicImage::ImageRgba8)
}

fn decode_cpt_gray_plane(bytes: &[u8], blk: &CptBlock) -> Option<Vec<u8>> {
    if blk.bpp != 8 {
        return None;
    }
    let w = blk.width as usize;
    let h = blk.height as usize;
    let tw = blk.tile_w.max(1) as usize;
    let th = blk.tile_h.max(1) as usize;
    let nx = w.div_ceil(tw);
    let ny = h.div_ceil(th);
    let num_tiles = nx.checked_mul(ny)?;
    if blk.tile_table_off.checked_add(num_tiles.checked_mul(8)?)? > bytes.len() {
        return None;
    }

    let mut out = vec![255u8; w * h];
    let tile_stride = (tw + 3) & !3;
    for ty in 0..ny {
        for tx in 0..nx {
            let tidx = ty * nx + tx;
            let entry_off = blk.tile_table_off + tidx * 8;
            let tile_off = read_u32_le(bytes, entry_off)? as usize;
            let tile_sz = read_u32_le(bytes, entry_off + 4)? as usize;
            if tile_off == 0 || tile_sz == 0 || tile_off.saturating_add(tile_sz) > bytes.len() {
                continue;
            }
            let cw = tw.min(w - tx * tw);
            let ch = th.min(h - ty * th);
            let cropped_stride = (cw + 3) & !3;
            let Some(raw) = decode_cpt9_tile(
                &bytes[tile_off..tile_off + tile_sz],
                tile_stride,
                cropped_stride,
                ch,
                1,
                tile_stride * th,
            ) else {
                continue;
            };
            let stride = if raw.len() >= tile_stride * ch {
                tile_stride
            } else if raw.len() >= cropped_stride * ch {
                cropped_stride
            } else {
                continue;
            };
            for y in 0..ch {
                let src_global_y = ty * th + y;
                if src_global_y >= h {
                    break;
                }
                let dst_y = h - 1 - src_global_y;
                for x in 0..cw {
                    out[dst_y * w + (tx * tw + x)] = raw[y * stride + x];
                }
            }
        }
    }
    Some(out)
}

/// Decodes a CPT9 compressed tile:
/// - 4-byte tile header: `count_bits: u16` (typically 5), `mode_bits: u16` (typically 3)
/// - LSB-first bitstream of `(count: count_bits, mode: mode_bits)` packets:
///   - `n = count + 1` samples
///   - `mode == 0` -> `0` bits/sample (n zero deltas)
///   - `mode > 0`  -> `(mode + 1)` bits/sample, sign-extended two's complement deltas
/// - Reconstruction predictor:
///   - Row `y == 0`: left-pixel predictor (`px[i] = px[i - bpp] + delta[i]`)
///   - Row `y > 0`:  up-row predictor (`px[y, i] = px[y - 1, i] + delta[y, i]`)
fn decode_cpt9_tile(
    tile: &[u8],
    tile_stride: usize,
    cropped_stride: usize,
    ch: usize,
    bytes_per_px: usize,
    expected_full: usize,
) -> Option<Vec<u8>> {
    if tile.len() < 4 {
        return None;
    }

    let cb = read_u16_le(tile, 0)? as u32;
    let mb = read_u16_le(tile, 2)? as u32;

    // Uncompressed raw tile fallback (if cb/mb header doesn't match bit-packed format)
    if !(1..=8).contains(&cb) || !(1..=4).contains(&mb) {
        if tile.len() >= cropped_stride * ch {
            return Some(tile.to_vec());
        }
        return None;
    }

    let hb = cb + mb;
    let cmask = (1u32 << cb) - 1;
    let mmask = (1u32 << mb) - 1;
    let max_bits = tile.len() * 8;
    let cap = expected_full.max(cropped_stride * ch).min(4 * 1024 * 1024);
    let mut deltas: Vec<i16> = Vec::with_capacity(cap);

    let mut bp: usize = 32; // skip 4-byte header
    while bp + (hb as usize) <= max_bits {
        let hdr = read_lsb_bits(tile, bp, hb);
        bp += hb as usize;
        let cnt = ((hdr & cmask) + 1) as usize;
        let mode = (hdr >> cb) & mmask;
        let bits: u32 = if mode == 0 {
            0
        } else {
            (mode + 1).min(8)
        };
        let need_bits = (bits as usize).saturating_mul(cnt);
        if bp + need_bits > max_bits {
            break;
        }
        if bits == 0 {
            deltas.resize(deltas.len() + cnt, 0);
        } else {
            let sign_bit = 1i32 << (bits - 1);
            let wrap = 1i32 << bits;
            for _ in 0..cnt {
                let raw = read_lsb_bits(tile, bp, bits) as i32;
                bp += bits as usize;
                let signed = if bits < 8 && (raw & sign_bit) != 0 {
                    raw - wrap
                } else {
                    raw
                };
                deltas.push(signed as i16);
            }
        }
        if deltas.len() >= cap + 65_536 {
            break;
        }
    }

    let stride = if deltas.len() >= tile_stride * ch {
        tile_stride
    } else if deltas.len() >= cropped_stride * ch {
        cropped_stride
    } else {
        return None;
    };

    let rows = (deltas.len() / stride).min(ch);
    let mut px = vec![0u8; rows * stride];

    for y in 0..rows {
        let row_off = y * stride;
        if y == 0 {
            for i in 0..stride {
                let pred = if i >= bytes_per_px {
                    px[i - bytes_per_px] as i32
                } else {
                    0
                };
                px[i] = ((pred + deltas[i] as i32) & 0xFF) as u8;
            }
        } else {
            let prev_off = (y - 1) * stride;
            for i in 0..stride {
                let pred = px[prev_off + i] as i32;
                px[row_off + i] = ((pred + deltas[row_off + i] as i32) & 0xFF) as u8;
            }
        }
    }

    Some(px)
}

#[inline]
fn read_lsb_bits(buf: &[u8], bit_pos: usize, count: u32) -> u32 {
    if count == 0 {
        return 0;
    }
    let byte_idx = bit_pos >> 3;
    let bit_off = bit_pos & 7;
    let b0 = *buf.get(byte_idx).unwrap_or(&0) as u32;
    let b1 = *buf.get(byte_idx + 1).unwrap_or(&0) as u32;
    let b2 = *buf.get(byte_idx + 2).unwrap_or(&0) as u32;
    let word = b0 | (b1 << 8) | (b2 << 16);
    (word >> bit_off) & ((1u32 << count) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn decodes_sample_cpt9_files_thumbnail_and_full() {
        let img_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("img");

        for (name, exp_w, exp_h, thumb_w, thumb_h) in [
            ("Golden Retriever 400.cpt", 507u32, 551u32, 235u32, 256u32),
            ("new-1.cpt", 243u32, 285u32, 127u32, 150u32),
        ] {
            let p = img_dir.join(name);
            if !p.is_file() {
                continue;
            }
            let bytes = std::fs::read(&p).expect("read sample cpt");

            // 1. Fast thumbnail decode (block 0x10)
            let thumb = parse_cpt_bytes(&bytes, true).expect("parse CPT thumb");
            assert_eq!(thumb.info.width, exp_w);
            assert_eq!(thumb.info.height, exp_h);
            assert_eq!(thumb.info.dpi_x, Some(72));
            let timg = thumb.image.expect("CPT thumbnail image must decode");
            assert_eq!(timg.width(), thumb_w);
            assert_eq!(timg.height(), thumb_h);

            // 2. Full-resolution decode (block 0x08)
            let full = parse_cpt_bytes(&bytes, false).expect("parse CPT full");
            let fimg = full.image.expect("CPT full image must decode");
            assert_eq!(fimg.width(), exp_w);
            assert_eq!(fimg.height(), exp_h);
        }
    }
}
