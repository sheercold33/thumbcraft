use image::{DynamicImage, ImageBuffer, Rgba};

const DDS_MAGIC: &[u8; 4] = b"DDS ";
const DDS_HEADER_SIZE: u32 = 124;

const DDSD_PITCH: u32 = 0x0000_0008;

const DDPF_ALPHAPIXELS: u32 = 0x0000_0001;
const DDPF_ALPHA: u32 = 0x0000_0002;
const DDPF_FOURCC: u32 = 0x0000_0004;
const DDPF_RGB: u32 = 0x0000_0040;
const DDPF_LUMINANCE: u32 = 0x0002_0000;
const DDPF_BUMPLUMINANCE: u32 = 0x0004_0000;
const DDPF_BUMPDUDV: u32 = 0x0008_0000;

const MAX_DDS_DIM: u32 = 32_768;
const MAX_DDS_PIXELS: u64 = 128_000_000;

#[derive(Debug, Clone)]
pub struct DdsHeaderInfo {
    pub width: u32,
    pub height: u32,
    pub mipmap_count: u32,
    pub format_label: String,
    pub bits_per_channel: String,
    pub is_compressed: bool,
    pub color_depth: u16,
}

#[derive(Debug, Clone)]
pub struct DecodedDds {
    pub image: DynamicImage,
    pub info: DdsHeaderInfo,
}

#[derive(Debug, Clone, Copy)]
struct PixelFormat {
    flags: u32,
    fourcc: [u8; 4],
    rgb_bit_count: u32,
    r_mask: u32,
    g_mask: u32,
    b_mask: u32,
    a_mask: u32,
}

#[derive(Debug, Clone, Copy)]
struct ParsedHeader {
    flags: u32,
    width: u32,
    height: u32,
    pitch_or_linear_size: u32,
    mipmap_count: u32,
    pf: PixelFormat,
    dxgi_format: Option<u32>,
    data_offset: usize,
}

#[inline]
fn read_u32_le(bytes: &[u8], off: usize) -> Result<u32, String> {
    let slice = bytes
        .get(off..off + 4)
        .ok_or_else(|| "Truncated DDS header".to_string())?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[inline]
fn read_u16_le(bytes: &[u8], off: usize) -> u16 {
    if off + 2 <= bytes.len() {
        u16::from_le_bytes([bytes[off], bytes[off + 1]])
    } else {
        0
    }
}

#[inline]
fn read_f32_le(bytes: &[u8], off: usize) -> f32 {
    if off + 4 <= bytes.len() {
        f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    } else {
        0.0
    }
}

fn parse_header(bytes: &[u8]) -> Result<ParsedHeader, String> {
    if bytes.len() < 128 {
        return Err("DDS file too small (< 128 bytes)".into());
    }
    if &bytes[0..4] != DDS_MAGIC {
        return Err("Missing 'DDS ' signature".into());
    }
    let header_size = read_u32_le(bytes, 4)?;
    if header_size != DDS_HEADER_SIZE {
        return Err(format!("Invalid DDS header size: {header_size}"));
    }

    let flags = read_u32_le(bytes, 8)?;
    let height = read_u32_le(bytes, 12)?;
    let width = read_u32_le(bytes, 16)?;
    let pitch_or_linear_size = read_u32_le(bytes, 20)?;
    let mipmap_count = read_u32_le(bytes, 28)?.max(1);

    if width == 0 || height == 0 {
        return Err(format!("Invalid DDS dimensions: {width}x{height}"));
    }
    if width > MAX_DDS_DIM || height > MAX_DDS_DIM || (width as u64) * (height as u64) > MAX_DDS_PIXELS {
        return Err(format!("DDS dimensions too large: {width}x{height}"));
    }

    let pf_flags = read_u32_le(bytes, 80)?;
    let fourcc = [bytes[84], bytes[85], bytes[86], bytes[87]];
    let rgb_bit_count = read_u32_le(bytes, 88)?;
    let r_mask = read_u32_le(bytes, 92)?;
    let g_mask = read_u32_le(bytes, 96)?;
    let b_mask = read_u32_le(bytes, 100)?;
    let a_mask = read_u32_le(bytes, 104)?;

    let pf = PixelFormat {
        flags: pf_flags,
        fourcc,
        rgb_bit_count,
        r_mask,
        g_mask,
        b_mask,
        a_mask,
    };

    let (dxgi_format, data_offset) = if (pf_flags & DDPF_FOURCC) != 0 && &fourcc == b"DX10" {
        if bytes.len() < 148 {
            return Err("Truncated DDS DX10 extended header".into());
        }
        let dxgi = read_u32_le(bytes, 128)?;
        (Some(dxgi), 148)
    } else {
        (None, 128)
    };

    Ok(ParsedHeader {
        flags,
        width,
        height,
        pitch_or_linear_size,
        mipmap_count,
        pf,
        dxgi_format,
        data_offset,
    })
}

/// Fast header-only inspection for DDS technical metadata.
pub fn inspect_dds_header(bytes: &[u8]) -> Option<DdsHeaderInfo> {
    let hdr = parse_header(bytes).ok()?;
    Some(describe_header(&hdr))
}

fn describe_header(hdr: &ParsedHeader) -> DdsHeaderInfo {
    let (format_label, bits_per_channel, is_compressed, color_depth) =
        if let Some(dxgi) = hdr.dxgi_format {
            describe_dxgi(dxgi)
        } else if (hdr.pf.flags & DDPF_FOURCC) != 0 && hdr.pf.fourcc != [0, 0, 0, 0] {
            describe_fourcc(hdr.pf.fourcc)
        } else {
            describe_uncompressed(&hdr.pf)
        };

    DdsHeaderInfo {
        width: hdr.width,
        height: hdr.height,
        mipmap_count: hdr.mipmap_count,
        format_label,
        bits_per_channel,
        is_compressed,
        color_depth,
    }
}

fn describe_uncompressed(pf: &PixelFormat) -> (String, String, bool, u16) {
    let bpp = pf.rgb_bit_count.clamp(8, 32) as u16;
    let label = match (pf.rgb_bit_count, pf.r_mask, pf.g_mask, pf.b_mask, pf.a_mask) {
        (32, 0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000) => "A8R8G8B8 (Uncompressed)".into(),
        (32, 0x00FF0000, 0x0000FF00, 0x000000FF, 0) => "X8R8G8B8 (Uncompressed)".into(),
        (32, 0x000000FF, 0x0000FF00, 0x00FF0000, 0xFF000000) => "A8B8G8R8 (Uncompressed)".into(),
        (32, 0x000000FF, 0x0000FF00, 0x00FF0000, 0) => "X8B8G8R8 (Uncompressed)".into(),
        (24, 0x00FF0000, 0x0000FF00, 0x000000FF, _) => "R8G8B8 (Uncompressed)".into(),
        (24, 0x000000FF, 0x0000FF00, 0x00FF0000, _) => "B8G8R8 (Uncompressed)".into(),
        (16, 0xF800, 0x07E0, 0x001F, 0) => "R5G6B5 (Uncompressed)".into(),
        (16, 0x7C00, 0x03E0, 0x001F, 0x8000) => "A1R5G5B5 (Uncompressed)".into(),
        (16, 0x7C00, 0x03E0, 0x001F, 0) => "X1R5G5B5 (Uncompressed)".into(),
        (16, 0x0F00, 0x00F0, 0x000F, 0xF000) => "A4R4G4B4 (Uncompressed)".into(),
        (16, 0x00FF, 0, 0, 0xFF00) if (pf.flags & DDPF_LUMINANCE) != 0 => {
            "A8L8 (Luminance+Alpha)".into()
        }
        (16, 0xFFFF, 0, 0, 0) if (pf.flags & DDPF_LUMINANCE) != 0 => "L16 (Luminance)".into(),
        (8, 0xFF, 0, 0, 0) if (pf.flags & DDPF_LUMINANCE) != 0 => "L8 (Luminance)".into(),
        (8, 0, 0, 0, 0xFF) if (pf.flags & DDPF_ALPHA) != 0 => "A8 (Alpha)".into(),
        (16, 0x00FF, 0xFF00, 0, 0) if (pf.flags & DDPF_BUMPDUDV) != 0 => "V8U8 (Bump Map)".into(),
        _ if (pf.flags & DDPF_BUMPDUDV) != 0 => format!("{bpp}-bit BumpDuDv"),
        _ if (pf.flags & DDPF_LUMINANCE) != 0 => format!("{bpp}-bit Luminance"),
        _ if (pf.flags & DDPF_ALPHA) != 0 => format!("{bpp}-bit Alpha"),
        _ => format!("{bpp}-bit Uncompressed"),
    };
    let max_ch_bits = [pf.r_mask, pf.g_mask, pf.b_mask, pf.a_mask]
        .into_iter()
        .map(|m| m.count_ones())
        .max()
        .unwrap_or(8);
    let bpc = if max_ch_bits == 0 {
        "8-bit".to_string()
    } else {
        format!("{max_ch_bits}-bit")
    };
    (label, bpc, false, bpp)
}

fn describe_fourcc(fourcc: [u8; 4]) -> (String, String, bool, u16) {
    match &fourcc {
        b"DXT1" => ("DXT1 (BC1)".into(), "8-bit".into(), true, 24),
        b"DXT2" => ("DXT2 (BC2 Premul)".into(), "8-bit".into(), true, 32),
        b"DXT3" => ("DXT3 (BC2)".into(), "8-bit".into(), true, 32),
        b"DXT4" => ("DXT4 (BC3 Premul)".into(), "8-bit".into(), true, 32),
        b"DXT5" => ("DXT5 (BC3)".into(), "8-bit".into(), true, 32),
        b"ATI1" | b"BC4U" => ("ATI1 (BC4 UNORM)".into(), "8-bit".into(), true, 8),
        b"BC4S" => ("BC4 SNORM".into(), "8-bit".into(), true, 8),
        b"ATI2" | b"BC5U" => ("ATI2 (BC5 Normal)".into(), "8-bit".into(), true, 24),
        b"BC5S" => ("BC5 SNORM".into(), "8-bit".into(), true, 24),
        b"YUY2" => ("YUY2 (YUV 4:2:2)".into(), "8-bit".into(), false, 16),
        b"UYVY" => ("UYVY (YUV 4:2:2)".into(), "8-bit".into(), false, 16),
        _ => match u32::from_le_bytes(fourcc) {
            36 => ("A16B16G16R16".into(), "16-bit".into(), false, 64),
            110 => ("Q16W16V16U16".into(), "16-bit".into(), false, 64),
            111 => ("R16F (Half Float)".into(), "16-bit (Float)".into(), false, 16),
            112 => ("G16R16F (Half Float)".into(), "16-bit (Float)".into(), false, 32),
            113 => ("A16B16G16R16F (Half Float)".into(), "16-bit (Float)".into(), false, 64),
            114 => ("R32F (Float)".into(), "32-bit (Float)".into(), false, 32),
            115 => ("G32R32F (Float)".into(), "32-bit (Float)".into(), false, 64),
            116 => ("A32B32G32R32F (Float)".into(), "32-bit (Float)".into(), false, 128),
            code => (
                format!("FourCC {:?} ({code})", String::from_utf8_lossy(&fourcc)),
                "8-bit".into(),
                true,
                32,
            ),
        },
    }
}

fn describe_dxgi(dxgi: u32) -> (String, String, bool, u16) {
    match dxgi {
        2 => ("DX10 R32G32B32A32_FLOAT".into(), "32-bit (Float)".into(), false, 128),
        6 => ("DX10 R32G32B32_FLOAT".into(), "32-bit (Float)".into(), false, 96),
        10 => ("DX10 R16G16B16A16_FLOAT".into(), "16-bit (Float)".into(), false, 64),
        11 => ("DX10 R16G16B16A16_UNORM".into(), "16-bit".into(), false, 64),
        13 => ("DX10 R16G16B16A16_SNORM".into(), "16-bit".into(), false, 64),
        16 => ("DX10 R32G32_FLOAT".into(), "32-bit (Float)".into(), false, 64),
        24 => ("DX10 R10G10B10A2_UNORM".into(), "10-bit".into(), false, 32),
        27..=29 => ("DX10 R8G8B8A8_UNORM".into(), "8-bit".into(), false, 32),
        31 => ("DX10 R8G8B8A8_SNORM".into(), "8-bit".into(), false, 32),
        34 => ("DX10 R16G16_FLOAT".into(), "16-bit (Float)".into(), false, 32),
        35 => ("DX10 R16G16_UNORM".into(), "16-bit".into(), false, 32),
        37 => ("DX10 R16G16_SNORM".into(), "16-bit".into(), false, 32),
        41 => ("DX10 R32_FLOAT".into(), "32-bit (Float)".into(), false, 32),
        49 => ("DX10 R8G8_UNORM".into(), "8-bit".into(), false, 16),
        51 => ("DX10 R8G8_SNORM".into(), "8-bit".into(), false, 16),
        54 => ("DX10 R16_FLOAT".into(), "16-bit (Float)".into(), false, 16),
        56 => ("DX10 R16_UNORM".into(), "16-bit".into(), false, 16),
        58 => ("DX10 R16_SNORM".into(), "16-bit".into(), false, 16),
        61 => ("DX10 R8_UNORM".into(), "8-bit".into(), false, 8),
        63 => ("DX10 R8_SNORM".into(), "8-bit".into(), false, 8),
        65 => ("DX10 A8_UNORM".into(), "8-bit".into(), false, 8),
        70..=72 => ("DX10 BC1 (DXT1)".into(), "8-bit".into(), true, 24),
        73..=75 => ("DX10 BC2 (DXT3)".into(), "8-bit".into(), true, 32),
        76..=78 => ("DX10 BC3 (DXT5)".into(), "8-bit".into(), true, 32),
        79..=81 => ("DX10 BC4".into(), "8-bit".into(), true, 8),
        82..=84 => ("DX10 BC5 (Normal)".into(), "8-bit".into(), true, 24),
        85 => ("DX10 B5G6R5_UNORM".into(), "5/6-bit".into(), false, 16),
        86 => ("DX10 B5G5R5A1_UNORM".into(), "5-bit".into(), false, 16),
        87 | 90 | 91 => ("DX10 B8G8R8A8_UNORM".into(), "8-bit".into(), false, 32),
        88 | 92 | 93 => ("DX10 B8G8R8X8_UNORM".into(), "8-bit".into(), false, 24),
        94..=96 => ("DX10 BC6H (HDR)".into(), "16-bit (Float)".into(), true, 48),
        97..=99 => ("DX10 BC7".into(), "8-bit".into(), true, 32),
        115 => ("DX10 B4G4R4A4_UNORM".into(), "4-bit".into(), false, 16),
        _ => (format!("DX10 DXGI_{dxgi}"), "8-bit".into(), false, 32),
    }
}

/// Decodes a DDS byte slice supporting uncompressed RGB/RGBA/Luminance/Alpha/BumpDuDv,
/// DXT1..5, ATI1/BC4, ATI2/BC5, BC7, YUY2/UYVY, half/full float, and DX10 headers.
pub fn decode_dds_bytes(bytes: &[u8]) -> Result<DecodedDds, String> {
    let hdr = parse_header(bytes)?;
    let info = describe_header(&hdr);
    let data = &bytes[hdr.data_offset..];
    let (w, h) = (hdr.width, hdr.height);

    let rgba: Vec<u8> = if let Some(dxgi) = hdr.dxgi_format {
        decode_dxgi(data, w, h, dxgi)?
    } else if (hdr.pf.flags & DDPF_FOURCC) != 0 && hdr.pf.fourcc != [0, 0, 0, 0] {
        decode_fourcc(data, w, h, hdr.pf.fourcc)?
    } else {
        decode_uncompressed(data, &hdr)?
    };

    let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, rgba)
        .ok_or_else(|| "Failed to build RGBA image buffer from DDS".to_string())?;

    Ok(DecodedDds {
        image: DynamicImage::ImageRgba8(img),
        info,
    })
}

// ---------------------------------------------------------------------------
// Uncompressed PixelFormat decoder (bitmask-driven: 8/16/24/32 bpp)
// ---------------------------------------------------------------------------

#[inline]
fn extract_unorm_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    if bits == 0 {
        return 0;
    }
    let raw = (pixel & mask) >> shift;
    if bits == 8 {
        raw as u8
    } else if bits < 8 {
        let max_v = (1u32 << bits) - 1;
        ((raw * 255 + max_v / 2) / max_v) as u8
    } else {
        // Scale down >8 bit channels (e.g. 10-bit or 16-bit) cleanly to 8-bit
        (raw >> (bits - 8)).min(255) as u8
    }
}

#[inline]
fn extract_snorm_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 128;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    if bits < 2 {
        return 128;
    }
    let raw = (pixel & mask) >> shift;
    let sign_bit = 1u32 << (bits - 1);
    let signed = if (raw & sign_bit) != 0 {
        let extend = !((1u32 << bits) - 1);
        (raw | extend) as i32
    } else {
        raw as i32
    };
    let max_pos = ((1i32 << (bits - 1)) - 1).max(1) as f32;
    let norm = (signed as f32 / max_pos).clamp(-1.0, 1.0);
    ((norm * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
}

fn decode_uncompressed(data: &[u8], hdr: &ParsedHeader) -> Result<Vec<u8>, String> {
    let pf = &hdr.pf;
    let bpp = if pf.rgb_bit_count > 0 {
        pf.rgb_bit_count
    } else {
        let used = pf.r_mask | pf.g_mask | pf.b_mask | pf.a_mask;
        if used == 0 {
            return Err("Uncompressed DDS has 0 bits per pixel and empty masks".into());
        }
        let highest = 32 - used.leading_zeros();
        if highest <= 8 {
            8
        } else if highest <= 16 {
            16
        } else if highest <= 24 {
            24
        } else {
            32
        }
    };

    if !matches!(bpp, 8 | 16 | 24 | 32) {
        return Err(format!("Unsupported uncompressed DDS bit depth: {bpp} bpp"));
    }

    let bytes_per_px = (bpp / 8) as usize;
    let w = hdr.width as usize;
    let h = hdr.height as usize;
    let packed_row = w
        .checked_mul(bytes_per_px)
        .ok_or_else(|| "DDS row size overflow".to_string())?;

    // Only trust dwPitchOrLinearSize if DDSD_PITCH is set, pitch >= packed_row, and buffer fits.
    let pitch = if (hdr.flags & DDSD_PITCH) != 0
        && (hdr.pitch_or_linear_size as usize) >= packed_row
        && (hdr.pitch_or_linear_size as usize)
            .checked_mul(h)
            .map(|n| n <= data.len())
            .unwrap_or(false)
    {
        hdr.pitch_or_linear_size as usize
    } else {
        packed_row
    };

    let min_needed = packed_row.saturating_mul(h);
    if data.len() < min_needed {
        return Err(format!(
            "Truncated uncompressed DDS pixel data (have {} bytes, need at least {})",
            data.len(),
            min_needed
        ));
    }

    let is_bump = (pf.flags & (DDPF_BUMPDUDV | DDPF_BUMPLUMINANCE)) != 0;
    let is_lum = (pf.flags & DDPF_LUMINANCE) != 0 && (pf.flags & DDPF_RGB) == 0;
    let is_alpha_only =
        (pf.flags & DDPF_ALPHA) != 0 && (pf.flags & (DDPF_RGB | DDPF_LUMINANCE)) == 0;
    let has_alpha = (pf.flags & DDPF_ALPHAPIXELS) != 0 || pf.a_mask != 0;

    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let row = &data[y * pitch..y * pitch + packed_row];
        let dst_row = &mut out[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            let p = x * bytes_per_px;
            let pixel: u32 = match bytes_per_px {
                1 => row[p] as u32,
                2 => u16::from_le_bytes([row[p], row[p + 1]]) as u32,
                3 => (row[p] as u32) | ((row[p + 1] as u32) << 8) | ((row[p + 2] as u32) << 16),
                _ => u32::from_le_bytes([row[p], row[p + 1], row[p + 2], row[p + 3]]),
            };

            let (r, g, b, a) = if is_bump {
                let du = extract_snorm_channel(pixel, pf.r_mask);
                let dv = extract_snorm_channel(pixel, pf.g_mask);
                let bw = if pf.b_mask != 0 {
                    extract_snorm_channel(pixel, pf.b_mask)
                } else {
                    255
                };
                let ba = if pf.a_mask != 0 {
                    extract_snorm_channel(pixel, pf.a_mask)
                } else {
                    255
                };
                (du, dv, bw, ba)
            } else if is_lum {
                let lum_mask = if pf.r_mask != 0 {
                    pf.r_mask
                } else if bpp == 16 && has_alpha {
                    0x00FF
                } else if bpp == 16 {
                    0xFFFF
                } else {
                    0x00FF
                };
                let l = extract_unorm_channel(pixel, lum_mask);
                let a = if has_alpha && pf.a_mask != 0 {
                    extract_unorm_channel(pixel, pf.a_mask)
                } else {
                    255
                };
                (l, l, l, a)
            } else if is_alpha_only {
                let amask = if pf.a_mask != 0 { pf.a_mask } else { 0xFF };
                let a = extract_unorm_channel(pixel, amask);
                (255, 255, 255, a)
            } else {
                // Standard RGB / RGBA (fallback to BGRA/BGR masks if all color masks were left 0)
                let (rm, gm, bm, am) = if (pf.r_mask | pf.g_mask | pf.b_mask) == 0 {
                    match bytes_per_px {
                        4 => (0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000),
                        3 => (0x00FF0000, 0x0000FF00, 0x000000FF, 0),
                        2 => (0xF800, 0x07E0, 0x001F, 0),
                        _ => (0xFF, 0xFF, 0xFF, 0),
                    }
                } else {
                    (pf.r_mask, pf.g_mask, pf.b_mask, pf.a_mask)
                };
                let r = extract_unorm_channel(pixel, rm);
                let g = extract_unorm_channel(pixel, gm);
                let b = extract_unorm_channel(pixel, bm);
                let a = if has_alpha && am != 0 {
                    extract_unorm_channel(pixel, am)
                } else {
                    255
                };
                (r, g, b, a)
            };

            let o = x * 4;
            dst_row[o] = r;
            dst_row[o + 1] = g;
            dst_row[o + 2] = b;
            dst_row[o + 3] = a;
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// FourCC & DXGI dispatch
// ---------------------------------------------------------------------------

fn decode_fourcc(data: &[u8], w: u32, h: u32, fourcc: [u8; 4]) -> Result<Vec<u8>, String> {
    match &fourcc {
        b"DXT1" => Ok(decode_bc1(data, w, h)),
        b"DXT2" => Ok(decode_bc2(data, w, h, true)),
        b"DXT3" => Ok(decode_bc2(data, w, h, false)),
        b"DXT4" => Ok(decode_bc3(data, w, h, true)),
        b"DXT5" => Ok(decode_bc3(data, w, h, false)),
        b"ATI1" | b"BC4U" => Ok(decode_bc4(data, w, h, false)),
        b"BC4S" => Ok(decode_bc4(data, w, h, true)),
        b"ATI2" | b"BC5U" => Ok(decode_bc5(data, w, h, false)),
        b"BC5S" => Ok(decode_bc5(data, w, h, true)),
        b"YUY2" => Ok(decode_yuy2(data, w, h, false)),
        b"UYVY" => Ok(decode_yuy2(data, w, h, true)),
        _ => match u32::from_le_bytes(fourcc) {
            36 => Ok(decode_rgba16_unorm(data, w, h)),
            110 => Ok(decode_rgba16_snorm(data, w, h)),
            111 => Ok(decode_r16f(data, w, h)),
            112 => Ok(decode_rg16f(data, w, h)),
            113 => Ok(decode_rgba16f(data, w, h)),
            114 => Ok(decode_r32f(data, w, h)),
            115 => Ok(decode_rg32f(data, w, h)),
            116 => Ok(decode_rgba32f(data, w, h)),
            code => Err(format!(
                "Unsupported DDS FourCC {:?} (code {code})",
                String::from_utf8_lossy(&fourcc)
            )),
        },
    }
}

fn decode_dxgi(data: &[u8], w: u32, h: u32, dxgi: u32) -> Result<Vec<u8>, String> {
    match dxgi {
        2 => Ok(decode_rgba32f(data, w, h)),
        6 => Ok(decode_rgb32f(data, w, h)),
        10 => Ok(decode_rgba16f(data, w, h)),
        11 => Ok(decode_rgba16_unorm(data, w, h)),
        13 => Ok(decode_rgba16_snorm(data, w, h)),
        16 => Ok(decode_rg32f(data, w, h)),
        24 => Ok(decode_r10g10b10a2_unorm(data, w, h)),
        27..=29 => Ok(decode_rgba8(data, w, h, false, true)),
        31 => Ok(decode_rgba8_snorm(data, w, h)),
        34 => Ok(decode_rg16f(data, w, h)),
        35 => Ok(decode_rg16_unorm(data, w, h)),
        37 => Ok(decode_rg16_snorm(data, w, h)),
        41 => Ok(decode_r32f(data, w, h)),
        49 => Ok(decode_rg8_unorm(data, w, h)),
        51 => Ok(decode_rg8_snorm(data, w, h)),
        54 => Ok(decode_r16f(data, w, h)),
        56 => Ok(decode_r16_unorm(data, w, h)),
        58 => Ok(decode_r16_snorm(data, w, h)),
        61 => Ok(decode_r8_unorm(data, w, h)),
        63 => Ok(decode_r8_snorm(data, w, h)),
        65 => Ok(decode_a8_unorm(data, w, h)),
        70..=72 => Ok(decode_bc1(data, w, h)),
        73..=75 => Ok(decode_bc2(data, w, h, false)),
        76..=78 => Ok(decode_bc3(data, w, h, false)),
        79 | 80 => Ok(decode_bc4(data, w, h, false)),
        81 => Ok(decode_bc4(data, w, h, true)),
        82 | 83 => Ok(decode_bc5(data, w, h, false)),
        84 => Ok(decode_bc5(data, w, h, true)),
        85 => Ok(decode_b5g6r5(data, w, h)),
        86 => Ok(decode_b5g5r5a1(data, w, h)),
        87 | 90 | 91 => Ok(decode_rgba8(data, w, h, true, true)),
        88 | 92 | 93 => Ok(decode_rgba8(data, w, h, true, false)),
        97..=99 => Ok(decode_bc7(data, w, h)),
        115 => Ok(decode_b4g4r4a4(data, w, h)),
        _ => Err(format!("Unsupported DDS DXGI format {dxgi}")),
    }
}

// ---------------------------------------------------------------------------
// Float & Uncompressed DXGI Helpers
// ---------------------------------------------------------------------------

#[inline]
fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 1) as u32;
    let exp = ((bits >> 10) & 0x1F) as i32;
    let mant = (bits & 0x03FF) as u32;
    let f_bits = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            let mut m = mant;
            let mut e = -14i32;
            while (m & 0x0400) == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x03FF;
            (sign << 31) | (((e + 127) as u32) << 23) | (m << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xFF << 23) | (mant << 13)
    } else {
        (sign << 31) | (((exp - 15 + 127) as u32) << 23) | (mant << 13)
    };
    f32::from_bits(f_bits)
}

#[inline]
fn linear_hdr_to_u8(v: f32) -> u8 {
    if !v.is_finite() || v <= 0.0 {
        return 0;
    }
    // Gentle Reinhard curve for HDR values > 1.0, linear/sRGB-like in [0, 1]
    let mapped = if v > 1.0 { v / (1.0 + v) } else { v };
    (mapped.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[inline]
fn snorm16_to_u8(v: i16) -> u8 {
    let n = (v as f32 / 32767.0).clamp(-1.0, 1.0);
    ((n * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
}

#[inline]
fn snorm8_to_u8(v: i8) -> u8 {
    let n = (v as f32 / 127.0).clamp(-1.0, 1.0);
    ((n * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
}

fn decode_rgba8(data: &[u8], w: u32, h: u32, bgra: bool, keep_alpha: bool) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![0u8; count * 4];
    for i in 0..count {
        let s = i * 4;
        if s + 4 > data.len() {
            break;
        }
        let (r, g, b, a) = if bgra {
            (data[s + 2], data[s + 1], data[s], data[s + 3])
        } else {
            (data[s], data[s + 1], data[s + 2], data[s + 3])
        };
        out[s] = r;
        out[s + 1] = g;
        out[s + 2] = b;
        out[s + 3] = if keep_alpha { a } else { 255 };
    }
    out
}

fn decode_rgba8_snorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 4;
        if s + 4 > data.len() {
            break;
        }
        out[s] = snorm8_to_u8(data[s] as i8);
        out[s + 1] = snorm8_to_u8(data[s + 1] as i8);
        out[s + 2] = snorm8_to_u8(data[s + 2] as i8);
        out[s + 3] = snorm8_to_u8(data[s + 3] as i8);
    }
    out
}

fn decode_rg8_unorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        out[i * 4] = data[s];
        out[i * 4 + 1] = data[s + 1];
        out[i * 4 + 2] = 0;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_rg8_snorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        out[i * 4] = snorm8_to_u8(data[s] as i8);
        out[i * 4 + 1] = snorm8_to_u8(data[s + 1] as i8);
        out[i * 4 + 2] = 255;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_r8_unorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count.min(data.len()) {
        let v = data[i];
        out[i * 4] = v;
        out[i * 4 + 1] = v;
        out[i * 4 + 2] = v;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_r8_snorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count.min(data.len()) {
        let v = snorm8_to_u8(data[i] as i8);
        out[i * 4] = v;
        out[i * 4 + 1] = v;
        out[i * 4 + 2] = v;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_a8_unorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count.min(data.len()) {
        out[i * 4 + 3] = data[i];
    }
    out
}

fn decode_b5g6r5(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        let px = read_u16_le(data, s) as u32;
        out[i * 4] = extract_unorm_channel(px, 0xF800);
        out[i * 4 + 1] = extract_unorm_channel(px, 0x07E0);
        out[i * 4 + 2] = extract_unorm_channel(px, 0x001F);
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_b5g5r5a1(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        let px = read_u16_le(data, s) as u32;
        out[i * 4] = extract_unorm_channel(px, 0x7C00);
        out[i * 4 + 1] = extract_unorm_channel(px, 0x03E0);
        out[i * 4 + 2] = extract_unorm_channel(px, 0x001F);
        out[i * 4 + 3] = if (px & 0x8000) != 0 { 255 } else { 0 };
    }
    out
}

fn decode_b4g4r4a4(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        let px = read_u16_le(data, s) as u32;
        out[i * 4] = extract_unorm_channel(px, 0x0F00);
        out[i * 4 + 1] = extract_unorm_channel(px, 0x00F0);
        out[i * 4 + 2] = extract_unorm_channel(px, 0x000F);
        out[i * 4 + 3] = extract_unorm_channel(px, 0xF000);
    }
    out
}

fn decode_r10g10b10a2_unorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 4;
        if s + 4 > data.len() {
            break;
        }
        let px = u32::from_le_bytes([data[s], data[s + 1], data[s + 2], data[s + 3]]);
        out[i * 4] = extract_unorm_channel(px, 0x0000_03FF);
        out[i * 4 + 1] = extract_unorm_channel(px, 0x000F_FC00);
        out[i * 4 + 2] = extract_unorm_channel(px, 0x3FF0_0000);
        out[i * 4 + 3] = extract_unorm_channel(px, 0xC000_0000);
    }
    out
}

fn decode_rgba16_unorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 8;
        if s + 8 > data.len() {
            break;
        }
        out[i * 4] = (read_u16_le(data, s) >> 8) as u8;
        out[i * 4 + 1] = (read_u16_le(data, s + 2) >> 8) as u8;
        out[i * 4 + 2] = (read_u16_le(data, s + 4) >> 8) as u8;
        out[i * 4 + 3] = (read_u16_le(data, s + 6) >> 8) as u8;
    }
    out
}

fn decode_rgba16_snorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 8;
        if s + 8 > data.len() {
            break;
        }
        out[i * 4] = snorm16_to_u8(read_u16_le(data, s) as i16);
        out[i * 4 + 1] = snorm16_to_u8(read_u16_le(data, s + 2) as i16);
        out[i * 4 + 2] = snorm16_to_u8(read_u16_le(data, s + 4) as i16);
        out[i * 4 + 3] = snorm16_to_u8(read_u16_le(data, s + 6) as i16);
    }
    out
}

fn decode_rg16_unorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 4;
        if s + 4 > data.len() {
            break;
        }
        out[i * 4] = (read_u16_le(data, s) >> 8) as u8;
        out[i * 4 + 1] = (read_u16_le(data, s + 2) >> 8) as u8;
        out[i * 4 + 2] = 0;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_rg16_snorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 4;
        if s + 4 > data.len() {
            break;
        }
        out[i * 4] = snorm16_to_u8(read_u16_le(data, s) as i16);
        out[i * 4 + 1] = snorm16_to_u8(read_u16_le(data, s + 2) as i16);
        out[i * 4 + 2] = 255;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_r16_unorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        let v = (read_u16_le(data, s) >> 8) as u8;
        out[i * 4] = v;
        out[i * 4 + 1] = v;
        out[i * 4 + 2] = v;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_r16_snorm(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        let v = snorm16_to_u8(read_u16_le(data, s) as i16);
        out[i * 4] = v;
        out[i * 4 + 1] = v;
        out[i * 4 + 2] = v;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_r16f(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        let v = linear_hdr_to_u8(f16_to_f32(read_u16_le(data, s)));
        out[i * 4] = v;
        out[i * 4 + 1] = v;
        out[i * 4 + 2] = v;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_rg16f(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 4;
        if s + 4 > data.len() {
            break;
        }
        out[i * 4] = linear_hdr_to_u8(f16_to_f32(read_u16_le(data, s)));
        out[i * 4 + 1] = linear_hdr_to_u8(f16_to_f32(read_u16_le(data, s + 2)));
        out[i * 4 + 2] = 0;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_rgba16f(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 8;
        if s + 8 > data.len() {
            break;
        }
        out[i * 4] = linear_hdr_to_u8(f16_to_f32(read_u16_le(data, s)));
        out[i * 4 + 1] = linear_hdr_to_u8(f16_to_f32(read_u16_le(data, s + 2)));
        out[i * 4 + 2] = linear_hdr_to_u8(f16_to_f32(read_u16_le(data, s + 4)));
        let af = f16_to_f32(read_u16_le(data, s + 6));
        out[i * 4 + 3] = if af.is_finite() {
            (af.clamp(0.0, 1.0) * 255.0).round() as u8
        } else {
            255
        };
    }
    out
}

fn decode_r32f(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 4;
        if s + 4 > data.len() {
            break;
        }
        let v = linear_hdr_to_u8(read_f32_le(data, s));
        out[i * 4] = v;
        out[i * 4 + 1] = v;
        out[i * 4 + 2] = v;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_rg32f(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 8;
        if s + 8 > data.len() {
            break;
        }
        out[i * 4] = linear_hdr_to_u8(read_f32_le(data, s));
        out[i * 4 + 1] = linear_hdr_to_u8(read_f32_le(data, s + 4));
        out[i * 4 + 2] = 0;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_rgb32f(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 12;
        if s + 12 > data.len() {
            break;
        }
        out[i * 4] = linear_hdr_to_u8(read_f32_le(data, s));
        out[i * 4 + 1] = linear_hdr_to_u8(read_f32_le(data, s + 4));
        out[i * 4 + 2] = linear_hdr_to_u8(read_f32_le(data, s + 8));
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_rgba32f(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let count = (w as usize) * (h as usize);
    let mut out = vec![255u8; count * 4];
    for i in 0..count {
        let s = i * 16;
        if s + 16 > data.len() {
            break;
        }
        out[i * 4] = linear_hdr_to_u8(read_f32_le(data, s));
        out[i * 4 + 1] = linear_hdr_to_u8(read_f32_le(data, s + 4));
        out[i * 4 + 2] = linear_hdr_to_u8(read_f32_le(data, s + 8));
        let af = read_f32_le(data, s + 12);
        out[i * 4 + 3] = if af.is_finite() {
            (af.clamp(0.0, 1.0) * 255.0).round() as u8
        } else {
            255
        };
    }
    out
}

fn decode_yuy2(data: &[u8], w: u32, h: u32, uyvy: bool) -> Vec<u8> {
    let w = w as usize;
    let h = h as usize;
    let mut out = vec![255u8; w * h * 4];
    let yuv_to_rgb = |y: u8, u: u8, v: u8| -> [u8; 3] {
        let c = y as f32 - 16.0;
        let d = u as f32 - 128.0;
        let e = v as f32 - 128.0;
        let r = (1.164 * c + 1.596 * e).round().clamp(0.0, 255.0) as u8;
        let g = (1.164 * c - 0.392 * d - 0.813 * e).round().clamp(0.0, 255.0) as u8;
        let b = (1.164 * c + 2.017 * d).round().clamp(0.0, 255.0) as u8;
        [r, g, b]
    };

    let row_bytes = w.div_ceil(2) * 4;
    for y in 0..h {
        let row_off = y * row_bytes;
        for pair in 0..w.div_ceil(2) {
            let s = row_off + pair * 4;
            if s + 4 > data.len() {
                break;
            }
            let (y0, u, y1, v) = if uyvy {
                (data[s + 1], data[s], data[s + 3], data[s + 2])
            } else {
                (data[s], data[s + 1], data[s + 2], data[s + 3])
            };
            let x0 = pair * 2;
            let rgb0 = yuv_to_rgb(y0, u, v);
            let o0 = (y * w + x0) * 4;
            out[o0..o0 + 3].copy_from_slice(&rgb0);
            if x0 + 1 < w {
                let rgb1 = yuv_to_rgb(y1, u, v);
                let o1 = (y * w + x0 + 1) * 4;
                out[o1..o1 + 3].copy_from_slice(&rgb1);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Block-Compressed Decoders: BC1 (DXT1), BC2 (DXT2/3), BC3 (DXT4/5), BC4, BC5
// ---------------------------------------------------------------------------

#[inline]
fn rgb565_to_rgb8(c: u16) -> [u8; 3] {
    let r5 = ((c >> 11) & 0x1F) as u8;
    let g6 = ((c >> 5) & 0x3F) as u8;
    let b5 = (c & 0x1F) as u8;
    [
        (r5 << 3) | (r5 >> 2),
        (g6 << 2) | (g6 >> 4),
        (b5 << 3) | (b5 >> 2),
    ]
}

#[inline]
fn decode_bc1_color_block(block: &[u8], punch_through_alpha: bool) -> [[u8; 4]; 16] {
    let c0_raw = u16::from_le_bytes([block[0], block[1]]);
    let c1_raw = u16::from_le_bytes([block[2], block[3]]);
    let c0 = rgb565_to_rgb8(c0_raw);
    let c1 = rgb565_to_rgb8(c1_raw);

    let mut pal = [[0u8; 4]; 4];
    pal[0] = [c0[0], c0[1], c0[2], 255];
    pal[1] = [c1[0], c1[1], c1[2], 255];

    if c0_raw > c1_raw || !punch_through_alpha {
        pal[2] = [
            ((2 * c0[0] as u16 + c1[0] as u16 + 1) / 3) as u8,
            ((2 * c0[1] as u16 + c1[1] as u16 + 1) / 3) as u8,
            ((2 * c0[2] as u16 + c1[2] as u16 + 1) / 3) as u8,
            255,
        ];
        pal[3] = [
            ((c0[0] as u16 + 2 * c1[0] as u16 + 1) / 3) as u8,
            ((c0[1] as u16 + 2 * c1[1] as u16 + 1) / 3) as u8,
            ((c0[2] as u16 + 2 * c1[2] as u16 + 1) / 3) as u8,
            255,
        ];
    } else {
        pal[2] = [
            ((c0[0] as u16 + c1[0] as u16) / 2) as u8,
            ((c0[1] as u16 + c1[1] as u16) / 2) as u8,
            ((c0[2] as u16 + c1[2] as u16) / 2) as u8,
            255,
        ];
        pal[3] = [0, 0, 0, 0];
    }

    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let mut pixels = [[0u8; 4]; 16];
    for (i, px) in pixels.iter_mut().enumerate() {
        let idx = ((indices >> (i * 2)) & 0x3) as usize;
        *px = pal[idx];
    }
    pixels
}

#[inline]
fn decode_bc4_channel_block(block: &[u8], signed: bool) -> [u8; 16] {
    let mut pal = [0u8; 8];
    if !signed {
        let a0 = block[0] as u16;
        let a1 = block[1] as u16;
        pal[0] = a0 as u8;
        pal[1] = a1 as u8;
        if a0 > a1 {
            for k in 1..=6u16 {
                pal[(k + 1) as usize] = (((7 - k) * a0 + k * a1 + 3) / 7) as u8;
            }
        } else {
            for k in 1..=4u16 {
                pal[(k + 1) as usize] = (((5 - k) * a0 + k * a1 + 2) / 5) as u8;
            }
            pal[6] = 0;
            pal[7] = 255;
        }
    } else {
        let a0 = block[0] as i8 as i32;
        let a1 = block[1] as i8 as i32;
        let to_u8 = |v: i32| -> u8 {
            let clamped = v.clamp(-127, 127) as f32 / 127.0;
            ((clamped * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
        };
        pal[0] = to_u8(a0);
        pal[1] = to_u8(a1);
        if a0 > a1 {
            for k in 1..=6i32 {
                pal[(k + 1) as usize] = to_u8(((7 - k) * a0 + k * a1) / 7);
            }
        } else {
            for k in 1..=4i32 {
                pal[(k + 1) as usize] = to_u8(((5 - k) * a0 + k * a1) / 5);
            }
            pal[6] = 0;
            pal[7] = 255;
        }
    }

    let mut bits: u64 = 0;
    for i in 0..6 {
        bits |= (block[2 + i] as u64) << (i * 8);
    }
    let mut vals = [0u8; 16];
    for (i, v) in vals.iter_mut().enumerate() {
        let idx = ((bits >> (i * 3)) & 0x7) as usize;
        *v = pal[idx];
    }
    vals
}

#[inline]
fn unpremultiply_pixel(px: &mut [u8; 4]) {
    let a = px[3];
    if a > 0 && a < 255 {
        let af = a as f32 / 255.0;
        px[0] = (px[0] as f32 / af).min(255.0).round() as u8;
        px[1] = (px[1] as f32 / af).min(255.0).round() as u8;
        px[2] = (px[2] as f32 / af).min(255.0).round() as u8;
    }
}

fn write_block_4x4(out: &mut [u8], w: usize, h: usize, bx: usize, by: usize, block_px: &[[u8; 4]; 16]) {
    let base_x = bx * 4;
    let base_y = by * 4;
    for py in 0..4 {
        let y = base_y + py;
        if y >= h {
            break;
        }
        for px in 0..4 {
            let x = base_x + px;
            if x >= w {
                break;
            }
            let o = (y * w + x) * 4;
            out[o..o + 4].copy_from_slice(&block_px[py * 4 + px]);
        }
    }
}

fn decode_bc1(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 8;
            if off + 8 > data.len() {
                return out;
            }
            let px = decode_bc1_color_block(&data[off..off + 8], true);
            write_block_4x4(&mut out, w, h, bx, by, &px);
        }
    }
    out
}

fn decode_bc2(data: &[u8], w: u32, h: u32, premul: bool) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 16;
            if off + 16 > data.len() {
                return out;
            }
            let alpha_bits = u64::from_le_bytes([
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
                data[off + 4],
                data[off + 5],
                data[off + 6],
                data[off + 7],
            ]);
            let mut px = decode_bc1_color_block(&data[off + 8..off + 16], false);
            for (i, p) in px.iter_mut().enumerate() {
                let a4 = ((alpha_bits >> (i * 4)) & 0xF) as u8;
                p[3] = (a4 << 4) | a4;
                if premul {
                    unpremultiply_pixel(p);
                }
            }
            write_block_4x4(&mut out, w, h, bx, by, &px);
        }
    }
    out
}

fn decode_bc3(data: &[u8], w: u32, h: u32, premul: bool) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 16;
            if off + 16 > data.len() {
                return out;
            }
            let alphas = decode_bc4_channel_block(&data[off..off + 8], false);
            let mut px = decode_bc1_color_block(&data[off + 8..off + 16], false);
            for i in 0..16 {
                px[i][3] = alphas[i];
                if premul {
                    unpremultiply_pixel(&mut px[i]);
                }
            }
            write_block_4x4(&mut out, w, h, bx, by, &px);
        }
    }
    out
}

fn decode_bc4(data: &[u8], w: u32, h: u32, signed: bool) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let mut out = vec![255u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 8;
            if off + 8 > data.len() {
                return out;
            }
            let ch = decode_bc4_channel_block(&data[off..off + 8], signed);
            let mut px = [[0u8; 4]; 16];
            for i in 0..16 {
                let v = ch[i];
                px[i] = [v, v, v, 255];
            }
            write_block_4x4(&mut out, w, h, bx, by, &px);
        }
    }
    out
}

fn decode_bc5(data: &[u8], w: u32, h: u32, signed: bool) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let mut out = vec![255u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 16;
            if off + 16 > data.len() {
                return out;
            }
            let r_ch = decode_bc4_channel_block(&data[off..off + 8], signed);
            let g_ch = decode_bc4_channel_block(&data[off + 8..off + 16], signed);
            let mut px = [[0u8; 4]; 16];
            for i in 0..16 {
                // Reconstruct normal-map Z channel so ATI2/BC5 normal maps render naturally
                let nx = (r_ch[i] as f32 / 255.0) * 2.0 - 1.0;
                let ny = (g_ch[i] as f32 / 255.0) * 2.0 - 1.0;
                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                let b = ((nz * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
                px[i] = [r_ch[i], g_ch[i], b, 255];
            }
            write_block_4x4(&mut out, w, h, bx, by, &px);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// BC7 Block Decoder (Modes 0–7, Khronos / Direct3D 11 Spec)
// ---------------------------------------------------------------------------

struct BitReader128 {
    lo: u64,
    hi: u64,
    pos: u32,
}

impl BitReader128 {
    fn from_block(b: &[u8]) -> Self {
        let lo = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        let hi = u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]);
        Self { lo, hi, pos: 0 }
    }

    #[inline]
    fn read(&mut self, bits: u32) -> u8 {
        if bits == 0 || self.pos >= 128 {
            return 0;
        }
        let p = self.pos;
        self.pos += bits;
        let val = if p + bits <= 64 {
            (self.lo >> p) & ((1u64 << bits) - 1)
        } else if p >= 64 {
            (self.hi >> (p - 64)) & ((1u64 << bits) - 1)
        } else {
            let lo_bits = 64 - p;
            let hi_bits = bits - lo_bits;
            let low = self.lo >> p;
            let high = self.hi & ((1u64 << hi_bits) - 1);
            low | (high << lo_bits)
        };
        val as u8
    }
}

const BC7_WEIGHTS_2: [u16; 4] = [0, 21, 43, 64];
const BC7_WEIGHTS_3: [u16; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const BC7_WEIGHTS_4: [u16; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

// 2-subset partitions (64 entries, 16 bits each: bit i = subset for texel i)
const BC7_PART2: [u16; 64] = [
    0xCCCC, 0x8888, 0xEEEE, 0xECC8, 0xC880, 0xFEEC, 0xFEC8, 0xEC80,
    0xC800, 0xFFEC, 0xFE80, 0xE800, 0xFFE8, 0xFF00, 0xFFF0, 0xF000,
    0xF710, 0x008E, 0x7100, 0x08CE, 0x008C, 0x7310, 0x3100, 0x8CCE,
    0x088C, 0x3110, 0x6666, 0x366C, 0x17E8, 0x0FF0, 0x718E, 0x399C,
    0xAAAA, 0xF0F0, 0x5A5A, 0x33CC, 0x3C3C, 0x55AA, 0x9696, 0xA55A,
    0x73CE, 0x13C8, 0x324C, 0x3BDC, 0x6996, 0xC33C, 0x9966, 0x0660,
    0x0272, 0x04E4, 0x4E40, 0x2720, 0xC936, 0x936C, 0x39C6, 0x639C,
    0x9336, 0x9CC6, 0x817E, 0xE718, 0xCCF0, 0x0FCC, 0x7744, 0xEE22,
];

// 3-subset partitions (64 entries, 32 bits each: 2 bits per texel i at shift 2*i)
const BC7_PART3: [u32; 64] = [
    0xAA68_5050, 0x6A5A_5040, 0x5A5A_4200, 0x5450_A0A8, 0xA5A5_0000, 0xA0A0_5050, 0x5555_A0A0, 0x5A5A_5050,
    0xAA55_0000, 0xAA55_5500, 0xAAAA_5500, 0x9090_9090, 0x9494_9494, 0xA4A4_A4A4, 0xA9A5_9450, 0x2A0A_4250,
    0xA594_5040, 0x0A42_5054, 0xA5A5_A500, 0x55A0_A0A0, 0xA8A8_5454, 0x6A6A_4040, 0xA4A4_5000, 0x1A1A_0500,
    0x0050_A4A4, 0xAAA5_9090, 0x1469_6914, 0x6969_1400, 0xA085_85A0, 0xAA82_1414, 0x50A4_A450, 0x6A5A_0200,
    0xA9A5_8000, 0x5090_A0A8, 0xA8A0_9050, 0x2424_2424, 0x00AA_5500, 0x2492_4924, 0x2449_9224, 0x50A5_0A50,
    0x500A_A550, 0xAAAA_4444, 0x6666_0000, 0xA5A0_A5A0, 0x50A0_50A0, 0x6928_6928, 0x44AA_AA44, 0x6666_6600,
    0xAA44_4444, 0x54A8_54A8, 0x9580_9580, 0x9696_9600, 0xA854_54A8, 0x8095_9580, 0xAA14_1414, 0x9696_0000,
    0xAAAA_1414, 0xA050_50A0, 0xA0A5_A5A0, 0x9600_0000, 0x4080_4080, 0xA9A8_A9A8, 0xAAAA_AA44, 0x2A4A_5254,
];

const BC7_ANCHOR_2: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,
    15,  2,  8,  2,  2,  8,  8, 15,  2,  8,  2,  2,  8,  8,  2,  2,
    15, 15,  6,  8,  2,  8, 15, 15,  2,  8,  2,  2,  2, 15, 15,  6,
     6,  2,  6,  8, 15, 15,  2,  2, 15, 15, 15, 15, 15,  2,  2, 15,
];

const BC7_ANCHOR_3A: [u8; 64] = [
     3,  3, 15, 15,  8,  3, 15, 15,  8,  8,  6,  6,  6,  5,  3,  3,
     3,  3,  8, 15,  3,  3,  6, 10,  5,  8,  8,  6,  8,  5, 15, 15,
     8, 15,  3,  5,  6, 10,  8, 15, 15,  3, 15,  5, 15, 15, 15, 15,
     3, 15,  5,  5,  5,  8,  5, 10,  5, 10,  8, 13, 15, 12,  3,  3,
];

const BC7_ANCHOR_3B: [u8; 64] = [
    15,  8,  8,  3, 15, 15,  3,  8, 15, 15, 15, 15, 15, 15, 15,  8,
    15,  8, 15,  3, 15,  8, 15,  8,  3, 15,  6, 10, 15, 15, 10,  8,
    15,  3, 15, 10, 10,  8,  9, 10,  6, 15,  8, 15,  3,  6,  6,  8,
    15,  3, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,  3, 15, 15,  8,
];

#[inline]
fn bc7_unquantize(val: u8, bits: u32) -> u8 {
    let shifted = (val as u32) << (8 - bits);
    (shifted | (shifted >> bits)) as u8
}

#[inline]
fn bc7_interp(e0: u8, e1: u8, weight: u16) -> u8 {
    (((64 - weight) * (e0 as u16) + weight * (e1 as u16) + 32) >> 6) as u8
}

fn decode_bc7_block(block: &[u8]) -> [[u8; 4]; 16] {
    let mut br = BitReader128::from_block(block);
    let mut mode = 0u32;
    while mode < 8 && br.read(1) == 0 {
        mode += 1;
    }
    if mode >= 8 {
        return [[0, 0, 0, 0]; 16];
    }

    // (ns, pb, rb, isb, cb, ab, epb, spb, ib, ib2)
    let (ns, pb, rb, isb, cb, ab, epb, spb, ib, ib2): (
        usize,
        u32,
        u32,
        u32,
        u32,
        u32,
        bool,
        bool,
        u32,
        u32,
    ) = match mode {
        0 => (3, 4, 0, 0, 4, 0, true, false, 3, 0),
        1 => (2, 6, 0, 0, 6, 0, false, true, 3, 0),
        2 => (3, 6, 0, 0, 5, 0, false, false, 2, 0),
        3 => (2, 6, 0, 0, 7, 0, true, false, 2, 0),
        4 => (1, 0, 2, 1, 5, 6, false, false, 2, 3),
        5 => (1, 0, 2, 0, 7, 8, false, false, 2, 2),
        6 => (1, 0, 0, 0, 7, 7, true, false, 4, 0),
        _ => (2, 6, 0, 0, 5, 5, true, false, 2, 0),
    };

    let part = br.read(pb) as usize;
    let rot = br.read(rb);
    let idx_sel = br.read(isb);

    let ne = ns * 2;
    let mut ep = [[0u8; 4]; 6]; // up to 3 subsets * 2 endpoints, RGBA

    for ch in 0..3 {
        for e in ep.iter_mut().take(ne) {
            e[ch] = br.read(cb);
        }
    }
    if ab > 0 {
        for e in ep.iter_mut().take(ne) {
            e[3] = br.read(ab);
        }
    } else {
        for e in ep.iter_mut().take(ne) {
            e[3] = 255;
        }
    }

    let c_bits = cb + u32::from(epb || spb);
    let a_bits = if ab > 0 {
        ab + u32::from(epb || spb)
    } else {
        0
    };

    if epb {
        for e in ep.iter_mut().take(ne) {
            let p = br.read(1);
            for ch in 0..3 {
                e[ch] = (e[ch] << 1) | p;
            }
            if ab > 0 {
                e[3] = (e[3] << 1) | p;
            }
        }
    } else if spb {
        for s in 0..ns {
            let p = br.read(1);
            for e in &mut ep[s * 2..s * 2 + 2] {
                for ch in 0..3 {
                    e[ch] = (e[ch] << 1) | p;
                }
            }
        }
    }

    for e in ep.iter_mut().take(ne) {
        for ch in 0..3 {
            e[ch] = bc7_unquantize(e[ch], c_bits);
        }
        if a_bits > 0 {
            e[3] = bc7_unquantize(e[3], a_bits);
        }
    }

    let subset_of = |i: usize| -> usize {
        match ns {
            1 => 0,
            2 => ((BC7_PART2[part & 63] >> i) & 1) as usize,
            _ => ((BC7_PART3[part & 63] >> (i * 2)) & 3) as usize,
        }
    };
    let is_anchor = |i: usize| -> bool {
        if i == 0 {
            return true;
        }
        match ns {
            2 => i == BC7_ANCHOR_2[part & 63] as usize,
            3 => i == BC7_ANCHOR_3A[part & 63] as usize || i == BC7_ANCHOR_3B[part & 63] as usize,
            _ => false,
        }
    };

    let mut idx1 = [0u8; 16];
    for i in 0..16 {
        let bits = if is_anchor(i) { ib - 1 } else { ib };
        idx1[i] = br.read(bits);
    }

    let mut idx2 = [0u8; 16];
    if ib2 > 0 {
        for i in 0..16 {
            let bits = if i == 0 { ib2 - 1 } else { ib2 };
            idx2[i] = br.read(bits);
        }
    }

    let weight_of = |idx: u8, bits: u32| -> u16 {
        match bits {
            2 => BC7_WEIGHTS_2[(idx & 3) as usize],
            3 => BC7_WEIGHTS_3[(idx & 7) as usize],
            _ => BC7_WEIGHTS_4[(idx & 15) as usize],
        }
    };

    let mut out = [[0u8; 4]; 16];
    for i in 0..16 {
        let s = subset_of(i).min(ns - 1);
        let e0 = ep[s * 2];
        let e1 = ep[s * 2 + 1];

        let (cw, aw) = if ib2 == 0 {
            let w = weight_of(idx1[i], ib);
            (w, w)
        } else if idx_sel == 0 {
            (weight_of(idx1[i], ib), weight_of(idx2[i], ib2))
        } else {
            (weight_of(idx2[i], ib2), weight_of(idx1[i], ib))
        };

        let mut r = bc7_interp(e0[0], e1[0], cw);
        let mut g = bc7_interp(e0[1], e1[1], cw);
        let mut b = bc7_interp(e0[2], e1[2], cw);
        let mut a = if ab > 0 {
            bc7_interp(e0[3], e1[3], aw)
        } else {
            255
        };

        match rot {
            1 => std::mem::swap(&mut r, &mut a),
            2 => std::mem::swap(&mut g, &mut a),
            3 => std::mem::swap(&mut b, &mut a),
            _ => {}
        }
        out[i] = [r, g, b, a];
    }
    out
}

fn decode_bc7(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 16;
            if off + 16 > data.len() {
                return out;
            }
            let px = decode_bc7_block(&data[off..off + 16]);
            write_block_4x4(&mut out, w, h, bx, by, &px);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dds_header(
        w: u32,
        h: u32,
        pf_flags: u32,
        fourcc: [u8; 4],
        bpp: u32,
        masks: [u32; 4],
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 128];
        buf[0..4].copy_from_slice(b"DDS ");
        buf[4..8].copy_from_slice(&124u32.to_le_bytes());
        buf[8..12].copy_from_slice(&0x1007u32.to_le_bytes()); // CAPS | HEIGHT | WIDTH | PIXELFORMAT
        buf[12..16].copy_from_slice(&h.to_le_bytes());
        buf[16..20].copy_from_slice(&w.to_le_bytes());
        buf[76..80].copy_from_slice(&32u32.to_le_bytes()); // pf size
        buf[80..84].copy_from_slice(&pf_flags.to_le_bytes());
        buf[84..88].copy_from_slice(&fourcc);
        buf[88..92].copy_from_slice(&bpp.to_le_bytes());
        buf[92..96].copy_from_slice(&masks[0].to_le_bytes());
        buf[96..100].copy_from_slice(&masks[1].to_le_bytes());
        buf[100..104].copy_from_slice(&masks[2].to_le_bytes());
        buf[104..108].copy_from_slice(&masks[3].to_le_bytes());
        buf
    }

    #[test]
    fn decodes_uncompressed_a8r8g8b8_and_r5g6b5_and_l8() {
        // 2x2 A8R8G8B8 (the exact kind of DDS that image-0.25 rejects with "DDS is not supported")
        let mut dds = make_dds_header(
            2,
            2,
            DDPF_RGB | DDPF_ALPHAPIXELS,
            [0; 4],
            32,
            [0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000],
        );
        // Pixel (0,0): R=250, G=100, B=20, A=200 in little-endian BGRA
        dds.extend_from_slice(&[20, 100, 250, 200]);
        dds.extend_from_slice(&[0, 255, 0, 255]);
        dds.extend_from_slice(&[255, 0, 0, 255]);
        dds.extend_from_slice(&[10, 20, 30, 128]);

        let dec = decode_dds_bytes(&dds).expect("A8R8G8B8 DDS must decode");
        assert_eq!(dec.info.width, 2);
        assert_eq!(dec.info.height, 2);
        assert!(dec.info.format_label.contains("A8R8G8B8"));
        let rgba = dec.image.to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [250, 100, 20, 200]);
        assert_eq!(rgba.get_pixel(1, 0).0, [0, 255, 0, 255]);
        assert_eq!(rgba.get_pixel(0, 1).0, [0, 0, 255, 255]);

        // R5G6B5 1x1 pure red
        let mut r565 = make_dds_header(1, 1, DDPF_RGB, [0; 4], 16, [0xF800, 0x07E0, 0x001F, 0]);
        r565.extend_from_slice(&0xF800u16.to_le_bytes());
        let dec2 = decode_dds_bytes(&r565).expect("R5G6B5 DDS must decode");
        assert_eq!(dec2.image.to_rgba8().get_pixel(0, 0).0, [255, 0, 0, 255]);

        // L8 1x1 mid-gray
        let mut l8 = make_dds_header(1, 1, DDPF_LUMINANCE, [0; 4], 8, [0xFF, 0, 0, 0]);
        l8.push(140);
        let dec3 = decode_dds_bytes(&l8).expect("L8 DDS must decode");
        assert_eq!(dec3.image.to_rgba8().get_pixel(0, 0).0, [140, 140, 140, 255]);
    }

    #[test]
    fn decodes_dxt1_ati2_and_dx10_rgba8_non_multiple_of_4() {
        // 3x3 DXT1 solid red block (c0 = 0xF800, c1 = 0x0000, all indices = 0)
        let mut dxt1 = make_dds_header(3, 3, DDPF_FOURCC, *b"DXT1", 0, [0; 4]);
        dxt1.extend_from_slice(&0xF800u16.to_le_bytes());
        dxt1.extend_from_slice(&0x0000u16.to_le_bytes());
        dxt1.extend_from_slice(&0u32.to_le_bytes());
        let dec = decode_dds_bytes(&dxt1).expect("NPOT DXT1 must decode");
        assert_eq!(dec.image.width(), 3);
        assert_eq!(dec.image.height(), 3);
        assert_eq!(dec.image.to_rgba8().get_pixel(2, 2).0, [255, 0, 0, 255]);

        // 4x4 ATI2 (BC5 normal map, flat normal (128, 128) -> Z ~= 255)
        let mut ati2 = make_dds_header(4, 4, DDPF_FOURCC, *b"ATI2", 0, [0; 4]);
        ati2.extend_from_slice(&[128, 128, 0, 0, 0, 0, 0, 0]);
        ati2.extend_from_slice(&[128, 128, 0, 0, 0, 0, 0, 0]);
        let dec_bc5 = decode_dds_bytes(&ati2).expect("ATI2/BC5 must decode");
        let p = dec_bc5.image.to_rgba8().get_pixel(0, 0).0;
        assert_eq!(p[0], 128);
        assert_eq!(p[1], 128);
        assert!(p[2] >= 250);

        // DX10 R8G8B8A8_UNORM (DXGI 28)
        let mut dx10 = make_dds_header(1, 1, DDPF_FOURCC, *b"DX10", 0, [0; 4]);
        dx10.extend_from_slice(&28u32.to_le_bytes()); // dxgi_format
        dx10.extend_from_slice(&3u32.to_le_bytes()); // 2D
        dx10.extend_from_slice(&0u32.to_le_bytes());
        dx10.extend_from_slice(&1u32.to_le_bytes());
        dx10.extend_from_slice(&0u32.to_le_bytes());
        dx10.extend_from_slice(&[11, 22, 33, 44]);
        let dec_dx10 = decode_dds_bytes(&dx10).expect("DX10 RGBA8 must decode");
        assert_eq!(dec_dx10.image.to_rgba8().get_pixel(0, 0).0, [11, 22, 33, 44]);
    }

    #[test]
    fn decodes_bc7_mode6_solid_color() {
        // BC7 Mode 6: 6 zero bits + 1 one bit = 0b0100_0000 (bit 6 = 1).
        // Endpoints: R0=127, R1=127, G0=0, G1=0, B0=64, B1=64, A0=127, A1=127, P0=1, P1=1, indices=0
        let mut bits = [0u8; 16];
        let mut bit_pos = 0usize;
        let mut push = |val: u8, n: usize| {
            for k in 0..n {
                if ((val >> k) & 1) != 0 {
                    bits[bit_pos / 8] |= 1 << (bit_pos % 8);
                }
                bit_pos += 1;
            }
        };
        push(0b100_0000, 7); // mode 6
        push(127, 7); // R0
        push(127, 7); // R1
        push(0, 7);   // G0
        push(0, 7);   // G1
        push(64, 7);  // B0
        push(64, 7);  // B1
        push(127, 7); // A0
        push(127, 7); // A1
        push(1, 1);   // P0
        push(1, 1);   // P1
        // All remaining index bits are 0

        let mut bc7 = make_dds_header(4, 4, DDPF_FOURCC, *b"DX10", 0, [0; 4]);
        bc7.extend_from_slice(&98u32.to_le_bytes()); // DXGI_FORMAT_BC7_UNORM
        bc7.extend_from_slice(&3u32.to_le_bytes());
        bc7.extend_from_slice(&0u32.to_le_bytes());
        bc7.extend_from_slice(&1u32.to_le_bytes());
        bc7.extend_from_slice(&0u32.to_le_bytes());
        bc7.extend_from_slice(&bits);

        let dec = decode_dds_bytes(&bc7).expect("BC7 must decode");
        let px = dec.image.to_rgba8().get_pixel(0, 0).0;
        assert_eq!(px[0], 255);
        assert!(px[1] <= 1);
        assert!((px[2] as i32 - 129).abs() <= 2);
        assert_eq!(px[3], 255);
    }
}
