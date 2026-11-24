use image::imageops::FilterType;
use image::DynamicImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

/// Default color weight contribution to visual similarity (40%)
pub const DEFAULT_COLOR_SIMILARITY_WEIGHT: f32 = 0.40;

/// Perceptual thumbnail fingerprint combining structural (dHash + luma) and color metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VisualSignature {
    /// 64-bit difference hash (horizontal gradient structure)
    pub dhash: u64,
    /// 8x8 grayscale luminance samples (64 bytes, 0..=255)
    pub luma_8x8: Vec<u8>,
    /// 4x4 RGB color grid (48 bytes: 16 cells * 3 channels)
    pub color_4x4: Vec<u8>,
}

impl VisualSignature {
    pub fn from_image(img: &DynamicImage) -> Self {
        // 1. 9x8 grayscale for 64-bit dHash + 8x8 structural luminance
        let gray9x8 = img.resize_exact(9, 8, FilterType::Triangle).to_luma8();
        let mut dhash: u64 = 0;
        let mut luma_8x8 = Vec::with_capacity(64);
        for y in 0..8u32 {
            for x in 0..8u32 {
                let left = gray9x8.get_pixel(x, y)[0];
                let right = gray9x8.get_pixel(x + 1, y)[0];
                if left > right {
                    dhash |= 1u64 << (y * 8 + x);
                }
                luma_8x8.push(left);
            }
        }

        // 2. 4x4 RGB color layout (48 bytes)
        let rgb4x4 = img.resize_exact(4, 4, FilterType::Triangle).to_rgb8();
        let color_4x4 = rgb4x4.into_raw();

        Self {
            dhash,
            luma_8x8,
            color_4x4,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.luma_8x8.len() == 64 && self.color_4x4.len() == 48
    }
}

/// Computes visual similarity in [0.0, 1.0] combining structural similarity and color similarity
/// according to `color_weight` (default 0.40, matching ThumbsPlus Preferences).
pub fn similarity_score(a: &VisualSignature, b: &VisualSignature, color_weight: f32) -> f32 {
    if !a.is_valid() || !b.is_valid() {
        return 0.0;
    }

    // Structural similarity: 50% dHash Hamming + 50% normalized 8x8 luminance L1 distance
    let hamming = (a.dhash ^ b.dhash).count_ones() as f32;
    let dhash_sim = 1.0 - (hamming / 64.0);

    let luma_diff: f32 = a
        .luma_8x8
        .iter()
        .zip(b.luma_8x8.iter())
        .map(|(&x, &y)| (x as i32 - y as i32).abs() as f32)
        .sum::<f32>()
        / (64.0 * 255.0);
    let luma_sim = 1.0 - luma_diff;
    let struct_sim = 0.5 * dhash_sim + 0.5 * luma_sim;

    // Color similarity: 4x4 RGB L1 distance
    let color_diff: f32 = a
        .color_4x4
        .iter()
        .zip(b.color_4x4.iter())
        .map(|(&x, &y)| (x as i32 - y as i32).abs() as f32)
        .sum::<f32>()
        / (48.0 * 255.0);
    let color_sim = 1.0 - color_diff;

    let cw = color_weight.clamp(0.0, 1.0);
    ((1.0 - cw) * struct_sim + cw * color_sim).clamp(0.0, 1.0)
}

/// Inline cache-friendly representation of `VisualSignature` (zero heap indirections)
/// used for fast nearest-neighbor visual clustering across thousands of items.
#[derive(Clone, Copy)]
struct CompactSig {
    item_idx: usize,
    dhash: u64,
    luma: [u8; 64],
    color: [u8; 48],
}

impl CompactSig {
    fn from_sig(item_idx: usize, sig: &VisualSignature) -> Option<Self> {
        if !sig.is_valid() {
            return None;
        }
        let mut luma = [0u8; 64];
        luma.copy_from_slice(&sig.luma_8x8[..64]);
        let mut color = [0u8; 48];
        color.copy_from_slice(&sig.color_4x4[..48]);
        Some(Self {
            item_idx,
            dhash: sig.dhash,
            luma,
            color,
        })
    }
}

#[inline(always)]
fn compact_distance_bounded(
    a: &CompactSig,
    b: &CompactSig,
    w_ham: u32,
    w_luma: u32,
    w_col: u32,
    limit: u32,
) -> u32 {
    let d_ham = (a.dhash ^ b.dhash).count_ones().saturating_mul(w_ham);
    if d_ham >= limit {
        return d_ham;
    }
    let mut col_l1 = 0u32;
    for i in 0..48 {
        col_l1 += (a.color[i] as i32 - b.color[i] as i32).unsigned_abs();
    }
    let d_hc = d_ham.saturating_add(col_l1.saturating_mul(w_col));
    if d_hc >= limit {
        return d_hc;
    }
    let mut luma_l1 = 0u32;
    for i in 0..64 {
        luma_l1 += (a.luma[i] as i32 - b.luma[i] as i32).unsigned_abs();
    }
    d_hc.saturating_add(luma_l1.saturating_mul(w_luma))
}

/// Orders `(item_idx, Option<VisualSignature>)` entries by visual similarity using a fast
/// greedy nearest-neighbor chain starting from `anchor_item_idx` (or the first valid image).
/// Visually similar images and design variants end up directly adjacent throughout the view;
/// items without visual signatures (e.g., non-media files) are placed at the end in original order.
pub fn order_by_visual_similarity(
    entries: &[(usize, Option<VisualSignature>)],
    anchor_item_idx: Option<usize>,
    color_weight: f32,
    reverse_chain: bool,
) -> Vec<usize> {
    let mut active: Vec<CompactSig> = Vec::with_capacity(entries.len());
    let mut without_sig: Vec<usize> = Vec::new();

    for &(idx, ref sig_opt) in entries {
        if let Some(cs) = sig_opt.as_ref().and_then(|s| CompactSig::from_sig(idx, s)) {
            active.push(cs);
        } else {
            without_sig.push(idx);
        }
    }

    if active.len() <= 1 {
        let mut out: Vec<usize> = active.into_iter().map(|c| c.item_idx).collect();
        out.extend(without_sig);
        return out;
    }

    let cw = color_weight.clamp(0.0, 1.0) as f64;
    // Scale continuous weights to u32 so integer distance exactly matches `1.0 - similarity_score`:
    //   d_norm = (1 - cw) * 0.5 * (ham / 64) + (1 - cw) * 0.5 * (luma_l1 / 16320) + cw * (col_l1 / 12240)
    // Multiply by 16_320 * 12_240? Too big; multiply by 4_000_000 keeps max sum <= 4_000_000 (well under u32::MAX).
    const SCALE: f64 = 4_000_000.0;
    let w_ham = (((1.0 - cw) * 0.5 / 64.0) * SCALE).round().max(1.0) as u32;
    let w_luma = (((1.0 - cw) * 0.5 / (64.0 * 255.0)) * SCALE)
        .round()
        .max(1.0) as u32;
    let w_col = ((cw / (48.0 * 255.0)) * SCALE).round() as u32;

    let start_pos = anchor_item_idx
        .and_then(|a| active.iter().position(|c| c.item_idx == a))
        .unwrap_or(0);

    let mut ordered: Vec<usize> = Vec::with_capacity(entries.len());
    let mut current = active.swap_remove(start_pos);
    ordered.push(current.item_idx);

    while !active.is_empty() {
        let best_pos = if active.len() >= 1024 {
            active
                .par_chunks(256)
                .enumerate()
                .map(|(chunk_i, chunk)| {
                    let base = chunk_i * 256;
                    let mut best_d = u32::MAX;
                    let mut best_i = base;
                    for (off, cand) in chunk.iter().enumerate() {
                        let d = compact_distance_bounded(&current, cand, w_ham, w_luma, w_col, best_d);
                        if d < best_d {
                            best_d = d;
                            best_i = base + off;
                        }
                    }
                    (best_d, best_i)
                })
                .reduce(
                    || (u32::MAX, 0usize),
                    |a, b| if a.0 <= b.0 { a } else { b },
                )
                .1
        } else {
            let mut best_d = u32::MAX;
            let mut best_i = 0usize;
            for (i, cand) in active.iter().enumerate() {
                let d = compact_distance_bounded(&current, cand, w_ham, w_luma, w_col, best_d);
                if d < best_d {
                    best_d = d;
                    best_i = i;
                }
            }
            best_i
        };

        current = active.swap_remove(best_pos);
        ordered.push(current.item_idx);
    }

    if reverse_chain {
        ordered.reverse();
    }
    ordered.extend(without_sig);
    ordered
}

/// Computes standard RFC 1321 MD5 hex digest for an in-memory byte slice.
pub fn md5_bytes(data: &[u8]) -> String {
    let mut hasher = Md5State::new();
    hasher.update(data);
    hasher.finalize_hex()
}

/// Computes MD5 from an in-memory byte slice only if `data.len() <= max_bytes`.
pub fn md5_bytes_capped(data: &[u8], max_bytes: u64) -> Option<String> {
    if (data.len() as u64) > max_bytes {
        return None;
    }
    Some(md5_bytes(data))
}

/// Computes standard RFC 1321 MD5 hex digest for a file (streaming in 64 KB chunks)
pub fn md5_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Md5State::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize_hex())
}

/// Computes MD5 only if the file size does not exceed `max_bytes`.
pub fn md5_file_capped(path: &Path, max_bytes: u64) -> Result<Option<String>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > max_bytes {
        return Ok(None);
    }
    md5_file(path).map(Some)
}

// Pure-Rust RFC 1321 MD5 implementation (zero external C/crate dependencies)
struct Md5State {
    a: u32,
    b: u32,
    c: u32,
    d: u32,
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Md5State {
    fn new() -> Self {
        Self {
            a: 0x6745_2301,
            b: 0xefcd_ab89,
            c: 0x98ba_dcfe,
            d: 0x1032_5476,
            buf: [0; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut input: &[u8]) {
        self.total_len = self.total_len.wrapping_add(input.len() as u64);
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(input.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&input[..take]);
            self.buf_len += take;
            input = &input[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.process_block(&block);
                self.buf_len = 0;
            }
        }
        while input.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&input[..64]);
            self.process_block(&block);
            input = &input[64..];
        }
        if !input.is_empty() {
            self.buf[..input.len()].copy_from_slice(input);
            self.buf_len = input.len();
        }
    }

    fn finalize_hex(mut self) -> String {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        let pad_len = if self.buf_len <= 56 {
            56 - self.buf_len
        } else {
            120 - self.buf_len
        };
        let zeros = [0u8; 64];
        if pad_len > 64 {
            self.update(&zeros);
            self.update(&zeros[..pad_len - 64]);
        } else {
            self.update(&zeros[..pad_len]);
        }
        self.update(&bit_len.to_le_bytes());

        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.a.to_le_bytes());
        out[4..8].copy_from_slice(&self.b.to_le_bytes());
        out[8..12].copy_from_slice(&self.c.to_le_bytes());
        out[12..16].copy_from_slice(&self.d.to_le_bytes());

        let mut hex = String::with_capacity(32);
        for b in out {
            use std::fmt::Write;
            let _ = write!(&mut hex, "{b:02x}");
        }
        hex
    }

    fn process_block(&mut self, block: &[u8; 64]) {
        const S: [u32; 64] = [
            7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20,
            5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
            6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
        ];
        const K: [u32; 64] = [
            0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
            0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
            0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
            0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
            0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
            0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
            0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
            0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
            0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
            0xeb86d391,
        ];

        let mut m = [0u32; 16];
        for i in 0..16 {
            let j = i * 4;
            m[i] = u32::from_le_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
        }

        let (mut a, mut b, mut c, mut d) = (self.a, self.b, self.c, self.d);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a
                .wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }

        self.a = self.a.wrapping_add(a);
        self.b = self.b.wrapping_add(b);
        self.c = self.c.wrapping_add(c);
        self.d = self.d.wrapping_add(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn md5_rfc1321_vectors() {
        let mut h = Md5State::new();
        h.update(b"");
        assert_eq!(h.finalize_hex(), "d41d8cd98f00b204e9800998ecf8427e");

        let mut h2 = Md5State::new();
        h2.update(b"abc");
        assert_eq!(h2.finalize_hex(), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn visual_similarity_ranks_closer_images_higher() {
        let img_a = DynamicImage::ImageRgba8(ImageBuffer::from_fn(64, 64, |x, y| {
            Rgba([(x * 4) as u8, (y * 4) as u8, 80, 255])
        }));
        let img_similar = DynamicImage::ImageRgba8(ImageBuffer::from_fn(64, 64, |x, y| {
            Rgba([((x * 4).saturating_add(10)) as u8, (y * 4) as u8, 85, 255])
        }));
        let img_diff = DynamicImage::ImageRgba8(ImageBuffer::from_fn(64, 64, |x, y| {
            Rgba([255 - (x * 4) as u8, 255 - (y * 4) as u8, 220, 255])
        }));

        let sa = VisualSignature::from_image(&img_a);
        let ss = VisualSignature::from_image(&img_similar);
        let sd = VisualSignature::from_image(&img_diff);

        let sim_close = similarity_score(&sa, &ss, DEFAULT_COLOR_SIMILARITY_WEIGHT);
        let sim_far = similarity_score(&sa, &sd, DEFAULT_COLOR_SIMILARITY_WEIGHT);
        assert!(sim_close > 0.90, "expected close match > 0.90, got {sim_close}");
        assert!(sim_close > sim_far + 0.30, "expected close {sim_close} >> far {sim_far}");
    }

    #[test]
    fn visual_similarity_sort_clusters_similar_images_together() {
        let make_solid = |r: u8, g: u8, b: u8| {
            let img = DynamicImage::ImageRgba8(ImageBuffer::from_fn(32, 32, |x, y| {
                Rgba([
                    r.saturating_add((x / 8) as u8),
                    g.saturating_add((y / 8) as u8),
                    b,
                    255,
                ])
            }));
            VisualSignature::from_image(&img)
        };

        // Interleaved red & blue variants plus one non-media item (None)
        let entries = vec![
            (0, Some(make_solid(220, 20, 20))),  // red 1
            (1, Some(make_solid(20, 30, 230))),  // blue 1
            (2, None),                           // non-media file
            (3, Some(make_solid(215, 25, 25))),  // red 2
            (4, Some(make_solid(25, 35, 225))),  // blue 2
            (5, Some(make_solid(225, 18, 18))),  // red 3
        ];

        let order = order_by_visual_similarity(&entries, Some(0), DEFAULT_COLOR_SIMILARITY_WEIGHT, false);
        // Anchor (0) first, followed immediately by the other two reds (3, 5), then the blues (1, 4), then non-media (2) at the end
        assert_eq!(order[0], 0);
        assert!(order[1..3].contains(&3) && order[1..3].contains(&5), "reds not clustered: {order:?}");
        assert!(order[3..5].contains(&1) && order[3..5].contains(&4), "blues not clustered: {order:?}");
        assert_eq!(order[5], 2);
    }
}
